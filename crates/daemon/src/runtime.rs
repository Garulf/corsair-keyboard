use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use bragi::Notification;
use engine::KeyPress;
use ipc::{DeviceState, Event, Reply, Request};
use k100::{InputEvent, InputSource, KeyStateTracker, Session};

use crate::config::ConfigDir;
use crate::conflict::{remove_ignored, running_conflicts};
use crate::controller::{Controller, DeviceLink};

const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
const INPUT_POLL: Duration = Duration::from_millis(100);

pub enum Command {
    Request(Request, Sender<Reply>),
    Subscribe(Sender<Reply>),
    Input(InputEvent),
    Reload,
    Shutdown,
}

pub struct Connection {
    pub link: Box<dyn DeviceLink>,
    pub model: String,
    pub firmware: String,
    pub input: Option<Box<dyn InputSource>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    NotFound,
    Blocked(Vec<String>),
    Failed(String),
}

pub trait Connector {
    fn connect(&mut self) -> Result<Connection, ConnectError>;
}

pub struct HidConnector {
    pub ignore_conflicts: Vec<String>,
}

impl Connector for HidConnector {
    fn connect(&mut self) -> Result<Connection, ConnectError> {
        let conflicts = remove_ignored(running_conflicts(), &self.ignore_conflicts);
        if !conflicts.is_empty() {
            return Err(ConnectError::Blocked(conflicts));
        }
        let opened = k100::hid::open_device().map_err(|error| match error {
            k100::hid::OpenError::NotFound => ConnectError::NotFound,
            other => ConnectError::Failed(other.to_string()),
        })?;
        let session =
            Session::start(opened.commands, opened.model).map_err(|error| ConnectError::Failed(error.to_string()))?;
        let firmware = format_firmware(session.info().firmware);
        Ok(Connection {
            link: Box::new(session),
            model: opened.model.name.to_owned(),
            firmware,
            input: Some(Box::new(opened.input)),
        })
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
) {
    let started = Instant::now();
    let mut subscribers: Vec<Sender<Reply>> = Vec::new();
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
            Ok(Command::Subscribe(events)) => subscribers.push(events),
            Ok(Command::Input(event)) => {
                if let InputEvent::KeyDown(led) = event {
                    presses.push(KeyPress { led, at: started.elapsed() });
                }
                broadcast(&mut subscribers, input_event(event));
            }
            Ok(Command::Reload) => {
                let (profiles, errors) = config.load_profiles();
                controller.reload(profiles, errors);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        if Instant::now() >= next_frame {
            controller.tick(started.elapsed(), &presses);
            presses.clear();
            next_frame += controller.frame_interval();
            if next_frame < Instant::now() {
                next_frame = Instant::now() + controller.frame_interval();
            }
        }

        if controller.wants_device() {
            if let Some(stop) = input_stop.take() {
                stop.store(true, Ordering::SeqCst);
            }
            if Instant::now() >= next_connect {
                next_connect = Instant::now() + RECONNECT_INTERVAL;
                match connector.connect() {
                    Ok(connection) => {
                        if let Some(input) = connection.input {
                            input_stop = Some(spawn_input(input, sender.clone()));
                        }
                        controller.attach(connection.link, &connection.model, connection.firmware, started.elapsed());
                    }
                    Err(ConnectError::Blocked(processes)) => {
                        controller.set_device_state(DeviceState::Blocked { processes })
                    }
                    Err(ConnectError::NotFound) => controller.set_device_state(DeviceState::Disconnected),
                    Err(ConnectError::Failed(reason)) => controller.set_device_state(DeviceState::Paused { reason }),
                }
            }
        }

        for event in controller.take_events() {
            broadcast(&mut subscribers, event);
        }
    }

    if let Some(stop) = input_stop {
        stop.store(true, Ordering::SeqCst);
    }
    controller.shutdown();
}

fn broadcast(subscribers: &mut Vec<Sender<Reply>>, event: Event) {
    subscribers.retain(|subscriber| subscriber.send(Reply::Event { event: event.clone() }).is_ok());
}

fn input_event(event: InputEvent) -> Event {
    match event {
        InputEvent::KeyDown(led) => Event::Key { led, pressed: true },
        InputEvent::KeyUp(led) => Event::Key { led, pressed: false },
        InputEvent::DialCw => Event::Dial { clockwise: true },
        InputEvent::DialCcw => Event::Dial { clockwise: false },
    }
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

    struct CountingLink(Arc<Mutex<usize>>);

    impl DeviceLink for CountingLink {
        fn write_frame(&mut self, _frame: &k100::Frame) -> Result<(), k100::DeviceError> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
        fn set_property(&mut self, _p: bragi::PropertyId, _v: u32) -> Result<(), k100::DeviceError> {
            Ok(())
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
        fn connect(&mut self) -> Result<Connection, ConnectError> {
            let next = if self.attempts.is_empty() { Ok(()) } else { self.attempts.remove(0) };
            next.map(|()| Connection {
                link: Box::new(CountingLink(self.frames.clone())),
                model: "K100".into(),
                firmware: "1.15.61".into(),
                input: None,
            })
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
        let controller = Controller::new(Settings::default(), vec![default_profile()], vec![]);
        let frames = Arc::new(Mutex::new(0));
        let connector = ScriptedConnector {
            attempts: vec![Err(ConnectError::Blocked(vec!["iCUE.exe".into()]))],
            frames: frames.clone(),
        };
        let (sender, receiver) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        sender.send(Command::Subscribe(events_tx)).unwrap();
        let loop_sender = sender.clone();
        let handle = std::thread::spawn(move || run(controller, config, receiver, loop_sender, connector));

        let first = events_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let blocked = DeviceState::Blocked { processes: vec!["iCUE.exe".into()] };
        assert_eq!(first, Reply::Event { event: Event::DeviceState { state: blocked } });
        let connected = events_rx.recv_timeout(Duration::from_secs(5)).unwrap();
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
}
