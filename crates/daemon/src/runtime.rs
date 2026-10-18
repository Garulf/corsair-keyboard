use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use bragi::Notification;
use engine::KeyPress;
use ipc::{DeviceState, Event, Reply, Request};
use k100::{Frame, InputEvent, InputSource, KeyStateTracker, Session};

use crate::config::ConfigDir;
use crate::conflict::{remove_ignored, running_conflicts};
use crate::controller::{Controller, DeviceLink, LinkInfo};
use crate::sdk_server::{SdkBridge, SdkCommand};

const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
const INPUT_POLL: Duration = Duration::from_millis(100);
pub const SUBSCRIBER_QUEUE: usize = 256;
pub const FRAME_EVENT_INTERVAL: Duration = Duration::from_millis(50);

pub enum Command {
    Request(Request, Sender<Reply>),
    Subscribe { events: SyncSender<Reply>, frames: bool },
    Input(InputEvent),
    Reload,
    Focus(Option<String>),
    Sdk(SdkCommand),
    Shutdown,
}

pub struct Connection {
    pub link: Box<dyn DeviceLink>,
    pub info: LinkInfo,
    pub input: Option<Box<dyn InputSource>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    NotFound,
    Blocked(Vec<String>),
    Failed(String),
}

pub trait Connector {
    fn connect(&mut self, ignore_conflicts: &[String]) -> Result<Connection, ConnectError>;
}

pub struct HidConnector;

impl Connector for HidConnector {
    fn connect(&mut self, ignore_conflicts: &[String]) -> Result<Connection, ConnectError> {
        let conflicts = remove_ignored(running_conflicts(), ignore_conflicts);
        if !conflicts.is_empty() {
            return Err(ConnectError::Blocked(conflicts));
        }
        let opened = k100::hid::open_device().map_err(|error| match error {
            k100::hid::OpenError::NotFound => ConnectError::NotFound,
            other => ConnectError::Failed(other.to_string()),
        })?;
        let session =
            Session::start(opened.commands, opened.model).map_err(|error| ConnectError::Failed(error.to_string()))?;
        let info = LinkInfo {
            model: opened.model.name.to_owned(),
            firmware: format_firmware(session.info().firmware),
            max_polling_rate: session.info().max_polling_rate,
            serial: opened.serial,
            layout: session.info().layout,
        };
        Ok(Connection { link: Box::new(session), info, input: Some(Box::new(opened.input)) })
    }
}

fn format_firmware(raw: u32) -> String {
    let [major, minor, low, high] = raw.to_le_bytes();
    format!("{major}.{minor}.{}", u16::from_le_bytes([low, high]))
}

pub fn run(
    mut controller: Controller,
    config: ConfigDir,
    commands: Receiver<Command>,
    sender: Sender<Command>,
    mut connector: impl Connector,
    sdk_enabled: Arc<AtomicBool>,
) {
    let mut sdk = SdkBridge::new(sdk::ServerConfig::default(), config.sdk_trace_path(), sdk_enabled);
    let started = Instant::now();
    let mut subscribers = Subscribers::default();
    let mut presses: Vec<KeyPress> = Vec::new();
    let mut input_stop: Option<Arc<AtomicBool>> = None;
    let mut next_frame = Instant::now();
    let mut next_connect = Instant::now();

    loop {
        let wait = next_frame.saturating_duration_since(Instant::now());
        match commands.recv_timeout(wait) {
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Command::Request(request, reply)) => {
                let _ = reply.send(controller.handle(request, &config));
            }
            Ok(Command::Subscribe { events, frames }) => subscribers.add(events, frames),
            Ok(Command::Input(event)) => {
                if let InputEvent::KeyDown(led) = event {
                    presses.push(KeyPress { led, at: started.elapsed() });
                }
                controller.input(event, started.elapsed(), &config);
                sdk.input(&event, &mut controller);
                if let Some(event) = input_event(event) {
                    subscribers.broadcast(event);
                }
            }
            Ok(Command::Reload) => {
                controller.reload_settings(config.load_settings());
                let (profiles, errors, failed) = config.load_profile_set();
                controller.reload(profiles, errors, failed);
            }
            Ok(Command::Focus(exe)) => controller.focus(exe),
            Ok(Command::Sdk(command)) => sdk.handle(command, &mut controller),
            Err(RecvTimeoutError::Timeout) => {}
        }

        if Instant::now() >= next_frame {
            controller.tick(started.elapsed(), &presses, &config);
            subscribers.publish_frame(controller.last_frame(), Instant::now());
            presses.clear();
            next_frame += controller.frame_interval();
            if next_frame < Instant::now() {
                next_frame = Instant::now() + controller.frame_interval();
            }
        }
        sdk.sync(&mut controller);

        if controller.wants_device() {
            if let Some(stop) = input_stop.take() {
                stop.store(true, Ordering::SeqCst);
            }
            if Instant::now() >= next_connect {
                next_connect = Instant::now() + RECONNECT_INTERVAL;
                match connector.connect(controller.ignore_conflicts()) {
                    Ok(connection) => {
                        if let Some(input) = connection.input {
                            input_stop = Some(spawn_input(input, sender.clone()));
                        }
                        controller.attach(connection.link, connection.info, started.elapsed());
                    }
                    Err(ConnectError::Blocked(processes)) => {
                        controller.set_device_state(DeviceState::Blocked { processes })
                    }
                    Err(ConnectError::NotFound) => controller.set_device_state(DeviceState::Disconnected),
                    Err(ConnectError::Failed(reason)) => controller.set_device_state(DeviceState::Paused { reason }),
                }
            }
        }

        sdk.sync(&mut controller);
        for event in controller.take_events() {
            subscribers.broadcast(event);
        }
    }

    if let Some(stop) = input_stop {
        stop.store(true, Ordering::SeqCst);
    }
    controller.shutdown();
}

struct Subscriber {
    events: SyncSender<Reply>,
    frames: bool,
}

#[derive(Default)]
struct Subscribers {
    list: Vec<Subscriber>,
    last_frame: Option<Frame>,
    next_frame: Option<Instant>,
}

impl Subscribers {
    fn add(&mut self, events: SyncSender<Reply>, frames: bool) {
        if frames {
            self.last_frame = None;
        }
        self.list.push(Subscriber { events, frames });
    }

    fn broadcast(&mut self, event: Event) {
        let reply = Reply::Event { event };
        self.list.retain(|subscriber| subscriber.events.try_send(reply.clone()).is_ok());
    }

    fn publish_frame(&mut self, frame: Option<Frame>, now: Instant) {
        let Some(frame) = frame else { return };
        if !self.list.iter().any(|subscriber| subscriber.frames) {
            return;
        }
        if self.next_frame.is_some_and(|due| now < due) || self.last_frame == Some(frame) {
            return;
        }
        self.next_frame = Some(now + FRAME_EVENT_INTERVAL);
        self.last_frame = Some(frame);
        let reply = Reply::Event { event: Event::Frame { rgb: ipc::encode_frame(&frame) } };
        self.list.retain(|subscriber| !subscriber.frames || subscriber.events.try_send(reply.clone()).is_ok());
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.list.len()
    }
}

fn input_event(event: InputEvent) -> Option<Event> {
    Some(match event {
        InputEvent::KeyDown(led) => Event::Key { led, pressed: true },
        InputEvent::KeyUp(led) => Event::Key { led, pressed: false },
        InputEvent::DialCw => Event::Dial { clockwise: true },
        InputEvent::DialCcw => Event::Dial { clockwise: false },
        InputEvent::Property { .. } => return None,
    })
}

fn spawn_input(mut input: Box<dyn InputSource>, sender: Sender<Command>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    thread::spawn(move || {
        let mut tracker = KeyStateTracker::default();
        while !flag.load(Ordering::SeqCst) {
            let report = match input.read(INPUT_POLL) {
                Ok(Some(report)) => report,
                Ok(None) => continue,
                Err(_) => break,
            };
            let events = Notification::parse(&report).map(|n| tracker.update(&n)).unwrap_or_default();
            for event in events {
                if sender.send(Command::Input(event)).is_err() {
                    return;
                }
            }
        }
    });
    stop
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::{Settings, default_profile};
    use crate::controller::{DiscardOutput, OutputPort};
    use crate::sdk_server::{OUTBOX_QUEUE, SdkCommand};

    struct CountingLink(Arc<Mutex<usize>>);

    impl DeviceLink for CountingLink {
        fn write_frame(&mut self, _frame: &k100::Frame) -> Result<(), k100::DeviceError> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
        fn write_indicators(&mut self, _records: &[k100::IndicatorRecord]) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn set_key_mappings(&mut self, _mappings: &k100::KeyMappings) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn get_property(&mut self, _p: bragi::PropertyId) -> Result<u32, k100::DeviceError> {
            Ok(0)
        }
        fn set_property(&mut self, _p: bragi::PropertyId, _v: u32) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn try_set_property(&mut self, _p: bragi::PropertyId, _v: u32) -> Result<bool, k100::DeviceError> {
            Ok(true)
        }
        fn key_count(&self) -> usize {
            138
        }
        fn ping(&mut self) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn release(&mut self) -> Result<(), k100::DeviceError> {
            Ok(())
        }
    }

    struct ScriptedConnector {
        attempts: Vec<Result<(), ConnectError>>,
        frames: Arc<Mutex<usize>>,
    }

    impl Connector for ScriptedConnector {
        fn connect(&mut self, _ignore: &[String]) -> Result<Connection, ConnectError> {
            let next = if self.attempts.is_empty() { Ok(()) } else { self.attempts.remove(0) };
            next.map(|()| Connection {
                link: Box::new(CountingLink(self.frames.clone())),
                info: LinkInfo {
                    model: "K100".into(),
                    firmware: "1.15.61".into(),
                    max_polling_rate: None,
                    serial: "SERIAL".into(),
                    layout: 1,
                },
                input: None,
            })
        }
    }

    fn gate() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(true))
    }

    #[derive(Clone, Default)]
    struct RecordingOutput(Arc<Mutex<Vec<actions::OutputCommand>>>);

    impl OutputPort for RecordingOutput {
        fn send(&mut self, command: actions::OutputCommand) {
            self.0.lock().unwrap().push(command);
        }

        fn is_busy(&self) -> bool {
            false
        }
    }

    impl RecordingOutput {
        fn presses_of(&self, source: u8) -> usize {
            let commands = self.0.lock().unwrap();
            commands
                .iter()
                .filter(|command| matches!(command, actions::OutputCommand::Press { source: s, .. } if *s == source))
                .count()
        }
    }

    fn request(sender: &mpsc::Sender<Command>, request: Request) -> Reply {
        let (reply_tx, reply_rx) = mpsc::channel();
        sender.send(Command::Request(request, reply_tx)).unwrap();
        reply_rx.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn reports_blocked_then_connects_and_renders() {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let controller = Controller::new(Settings::default(), vec![default_profile()], vec![], Box::new(DiscardOutput));
        let frames = Arc::new(Mutex::new(0));
        let connector = ScriptedConnector {
            attempts: vec![Err(ConnectError::Blocked(vec!["iCUE.exe".into()]))],
            frames: frames.clone(),
        };
        let (sender, receiver) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        sender.send(Command::Subscribe { events: events_tx, frames: false }).unwrap();
        let loop_sender = sender.clone();
        let handle = std::thread::spawn(move || run(controller, config, receiver, loop_sender, connector, gate()));

        let first = events_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let blocked = DeviceState::Blocked { processes: vec!["iCUE.exe".into()] };
        assert_eq!(first, Reply::Event { event: Event::DeviceState { state: blocked } });
        let connected = loop {
            let reply = events_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            if matches!(reply, Reply::Event { event: Event::DeviceState { .. } }) {
                break reply;
            }
        };
        assert!(matches!(
            connected,
            Reply::Event { event: Event::DeviceState { state: DeviceState::Connected { .. } } }
        ));
        std::thread::sleep(Duration::from_millis(200));
        assert!(*frames.lock().unwrap() > 3);
        assert!(matches!(request(&sender, Request::GetState), Reply::State { .. }));

        sender.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn stuck_subscribers_are_dropped_instead_of_buffering_forever() {
        let (stuck_tx, _stuck_rx) = mpsc::sync_channel(2);
        let (live_tx, live_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        let mut subscribers = Subscribers::default();
        subscribers.add(stuck_tx, false);
        subscribers.add(live_tx, false);
        for _ in 0..5 {
            subscribers.broadcast(Event::Dial { clockwise: true });
            while live_rx.try_recv().is_ok() {}
        }
        assert_eq!(subscribers.len(), 1);
    }

    #[test]
    fn identical_frames_are_not_resent_and_the_interval_is_respected() {
        let (tx, rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        let mut subscribers = Subscribers::default();
        subscribers.add(tx, true);
        let start = Instant::now();
        let mut frame = [k100::Rgb::BLACK; k100::LED_COUNT];
        subscribers.publish_frame(Some(frame), start);
        assert_eq!(rx.try_iter().count(), 1);
        frame[0] = k100::Rgb::new(1, 1, 1);
        subscribers.publish_frame(Some(frame), start + Duration::from_millis(10));
        assert_eq!(rx.try_iter().count(), 0, "too soon");
        subscribers.publish_frame(Some(frame), start + Duration::from_millis(60));
        assert_eq!(rx.try_iter().count(), 1);
        subscribers.publish_frame(Some(frame), start + Duration::from_millis(200));
        assert_eq!(rx.try_iter().count(), 0, "unchanged");
    }

    #[test]
    fn a_late_frame_subscriber_gets_the_current_frame() {
        let mut subscribers = Subscribers::default();
        let (first_tx, first_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        subscribers.add(first_tx, true);
        let start = Instant::now();
        let frame = [k100::Rgb::new(9, 9, 9); k100::LED_COUNT];
        subscribers.publish_frame(Some(frame), start);
        let (late_tx, late_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        subscribers.add(late_tx, true);
        subscribers.publish_frame(Some(frame), start + Duration::from_millis(60));
        assert_eq!(late_rx.try_iter().count(), 1);
        assert_eq!(first_rx.try_iter().count(), 2);
    }

    #[test]
    fn a_stalled_frame_subscriber_is_dropped() {
        let (stuck_tx, _stuck_rx) = mpsc::sync_channel(1);
        let mut subscribers = Subscribers::default();
        subscribers.add(stuck_tx, true);
        let start = Instant::now();
        for step in 0..4u8 {
            let frame = [k100::Rgb::new(step, 0, 0); k100::LED_COUNT];
            subscribers.publish_frame(Some(frame), start + Duration::from_millis(60 * u64::from(step)));
        }
        assert_eq!(subscribers.len(), 0);
    }

    #[test]
    fn frame_subscribers_get_throttled_frames_and_plain_subscribers_get_none() {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let controller = Controller::new(Settings::default(), vec![default_profile()], vec![], Box::new(DiscardOutput));
        let connector = ScriptedConnector { attempts: vec![], frames: Arc::new(Mutex::new(0)) };
        let (sender, receiver) = mpsc::channel();
        let (frames_tx, frames_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        let (plain_tx, plain_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        sender.send(Command::Subscribe { events: frames_tx, frames: true }).unwrap();
        sender.send(Command::Subscribe { events: plain_tx, frames: false }).unwrap();
        let loop_sender = sender.clone();
        let started = Instant::now();
        let handle = std::thread::spawn(move || run(controller, config, receiver, loop_sender, connector, gate()));
        std::thread::sleep(Duration::from_millis(600));
        sender.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
        let elapsed = started.elapsed();
        let is_frame = |reply: &Reply| matches!(reply, Reply::Event { event: Event::Frame { .. } });
        let frames = frames_rx.try_iter().filter(is_frame).count();
        let plain = plain_rx.try_iter().filter(is_frame).count();
        let ceiling = (elapsed.as_millis() / FRAME_EVENT_INTERVAL.as_millis()) as usize + 1;
        assert!((2..=ceiling).contains(&frames), "{frames} frames in {elapsed:?}");
        assert_eq!(plain, 0);
    }

    #[test]
    fn a_client_that_hangs_up_is_cleaned_up_through_the_runtime() {
        const DEVICE: &str = "{00000000-0000-4000-8000-000000000001}";
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let settings = Settings {
            sdk: ipc::SdkSettings { device_id: Some(DEVICE.into()), ..ipc::SdkSettings::default() },
            ..Settings::default()
        };
        let macro_g1 = actions::Assignment {
            key: actions::Key::named("g1").unwrap(),
            action: actions::Action::Macro {
                steps: vec![actions::MacroStep::Text("g1".into())],
                mode: actions::RepeatMode::Once,
            },
            pass_through: vec![],
        };
        let profile = ipc::Profile { assignments: vec![macro_g1], ..default_profile() };
        let output = RecordingOutput::default();
        let controller = Controller::new(settings, vec![profile], vec![], Box::new(output.clone()));
        let connector = ScriptedConnector { attempts: vec![], frames: Arc::new(Mutex::new(0)) };
        let (sender, receiver) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        sender.send(Command::Subscribe { events: events_tx, frames: false }).unwrap();
        let loop_sender = sender.clone();
        let handle = std::thread::spawn(move || run(controller, config, receiver, loop_sender, connector, gate()));
        while !matches!(
            events_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Reply::Event { event: Event::DeviceState { state: DeviceState::Connected { .. } } }
        ) {}

        let (outbox, replies) = mpsc::sync_channel(OUTBOX_QUEUE);
        let hang_up = Arc::new(|| {});
        sender
            .send(Command::Sdk(SdkCommand::Connected { client: 1, exe: "StreamDeck.exe".into(), outbox, hang_up }))
            .unwrap();
        let messages = [
            r#"{"method":0,"code":-1,"params":{"pid":1,"clientVersion":"4.0.84"}}"#.to_owned(),
            format!(r#"{{"method":12,"code":1,"params":{{"deviceId":"{DEVICE}","accessLevel":3}}}}"#),
            format!(r#"{{"method":16,"code":2,"params":{{"deviceId":"{DEVICE}","keyId":1,"isIntercepted":true}}}}"#),
        ];
        for message in messages {
            sender.send(Command::Sdk(SdkCommand::Message { client: 1, payload: message.into_bytes() })).unwrap();
            let reply: serde_json::Value =
                serde_json::from_slice(&replies.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
            assert_eq!(reply["error"], 0, "{reply}");
        }
        let tap = |sender: &mpsc::Sender<Command>| {
            sender.send(Command::Input(InputEvent::KeyDown(k100::led::G1))).unwrap();
            sender.send(Command::Input(InputEvent::KeyUp(k100::led::G1))).unwrap();
        };
        tap(&sender);
        let Reply::State { state } = request(&sender, Request::GetState) else { panic!("expected the state") };
        assert_eq!(state.sdk.clients[0].intercepted, ["G1"]);
        assert_eq!(output.presses_of(k100::led::G1), 0);

        sender.send(Command::Sdk(SdkCommand::Disconnected { client: 1 })).unwrap();
        tap(&sender);
        let Reply::State { state } = request(&sender, Request::GetState) else { panic!("expected the state") };
        assert!(state.sdk.clients.is_empty());
        assert_eq!(output.presses_of(k100::led::G1), 1);

        sender.send(Command::Shutdown).unwrap();
        handle.join().unwrap();
    }
}
