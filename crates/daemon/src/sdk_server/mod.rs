pub mod client;
mod host;
pub mod listener;
pub mod security;
pub mod trace;
#[cfg(windows)]
pub mod windows;

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;

use k100::InputEvent;
use sdk::{ClientId, Output, Server, ServerConfig};

use crate::config::{ConfigDir, Settings};
use crate::controller::Controller;
use crate::sdk_server::trace::{Direction, Trace};

pub const OUTBOX_QUEUE: usize = 256;

pub enum SdkCommand {
    Status(ipc::SdkStatus),
    Connected { client: ClientId, exe: String, outbox: SyncSender<Vec<u8>> },
    Message { client: ClientId, payload: Vec<u8> },
    Disconnected { client: ClientId },
}

struct Outbox {
    exe: String,
    sender: SyncSender<Vec<u8>>,
}

pub struct SdkBridge {
    server: Server,
    outboxes: HashMap<ClientId, Outbox>,
    trace: Trace,
    enabled: Arc<AtomicBool>,
}

impl SdkBridge {
    pub fn new(config: ServerConfig, trace_path: PathBuf, enabled: Arc<AtomicBool>) -> Self {
        Self { server: Server::new(config), outboxes: HashMap::new(), trace: Trace::new(trace_path), enabled }
    }

    pub fn handle(&mut self, command: SdkCommand, controller: &mut Controller) {
        match command {
            SdkCommand::Status(status) => controller.set_sdk_status(status),
            SdkCommand::Connected { client, exe, outbox } => {
                self.outboxes.insert(client, Outbox { exe: exe.clone(), sender: outbox });
                self.server.connect(client, exe, controller);
            }
            SdkCommand::Message { client, payload } => {
                self.trace(controller, client, Direction::In, &payload);
                let outputs = self.server.receive(client, &payload, controller);
                self.deliver(outputs, controller);
            }
            SdkCommand::Disconnected { client } => self.drop_client(client, controller),
        }
    }

    pub fn input(&mut self, event: &InputEvent, controller: &mut Controller) {
        let (led, pressed) = match *event {
            InputEvent::KeyDown(led) => (led, true),
            InputEvent::KeyUp(led) => (led, false),
            _ => return,
        };
        let outputs = self.server.key(led, pressed, controller);
        self.deliver(outputs, controller);
    }

    pub fn sync(&mut self, controller: &mut Controller) {
        self.enabled.store(controller.sdk_settings().enabled, Ordering::SeqCst);
        let outputs = self.server.sync(controller);
        self.deliver(outputs, controller);
    }

    fn deliver(&mut self, outputs: Vec<Output>, controller: &mut Controller) {
        for output in outputs {
            match output {
                Output::Send { client, payload } => {
                    self.trace(controller, client, Direction::Out, &payload);
                    let undeliverable =
                        self.outboxes.get(&client).is_some_and(|outbox| outbox.sender.try_send(payload).is_err());
                    if undeliverable {
                        self.drop_client(client, controller);
                    }
                }
                Output::Close { client } => self.drop_client(client, controller),
            }
        }
    }

    fn drop_client(&mut self, client: ClientId, controller: &mut Controller) {
        self.outboxes.remove(&client);
        self.server.disconnect(client, controller);
    }

    fn trace(&mut self, controller: &Controller, client: ClientId, direction: Direction, payload: &[u8]) {
        if !controller.sdk_settings().trace {
            self.trace.close();
            return;
        }
        if let Some(outbox) = self.outboxes.get(&client) {
            self.trace.record(client, &outbox.exe, direction, payload);
        }
    }
}

pub fn new_device_id() -> String {
    let high = RandomState::new().hash_one(0u8) & !0xF000 | 0x4000;
    let low = RandomState::new().hash_one(1u8) & !(0xC << 60) | (0x8 << 60);
    let hex = format!("{high:016x}{low:016x}");
    format!("{{{}-{}-{}-{}-{}}}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

pub fn with_device_id(config: &ConfigDir, mut settings: Settings) -> Settings {
    if settings.sdk.device_id.is_some() {
        return settings;
    }
    settings.sdk.device_id = Some(new_device_id());
    if let Err(error) = config.save_settings(&settings) {
        eprintln!("k100d: could not save the SDK device id: {error}");
    }
    settings
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
    use std::time::Duration;

    use ipc::{SdkSettings, SdkStatus};
    use k100::{InputEvent, led};
    use serde_json::Value;

    use super::*;
    use crate::config::{ConfigDir, Settings};
    use crate::controller::{Controller, DeviceLink, DiscardOutput, LinkInfo};

    const HANDSHAKE: &[u8] = br#"{"method":0,"code":-1,"params":{"pid":1,"clientVersion":"4.0.84"}}"#;
    const DEVICE: &str = "{00000000-0000-4000-8000-000000000001}";
    const SUBSCRIBE: &[u8] = br#"{"method":14,"code":5}"#;
    const DEVICE_EVENT: i64 = 1;
    const KEY_EVENT: i64 = 2;

    struct NullLink;

    impl DeviceLink for NullLink {
        fn write_frame(&mut self, _frame: &k100::Frame) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn write_indicators(&mut self, _records: &[k100::IndicatorRecord]) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn set_key_mappings(&mut self, _mappings: &k100::KeyMappings) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn get_property(&mut self, _property: bragi::PropertyId) -> Result<u32, k100::DeviceError> {
            Ok(0)
        }
        fn set_property(&mut self, _property: bragi::PropertyId, _value: u32) -> Result<(), k100::DeviceError> {
            Ok(())
        }
        fn try_set_property(&mut self, _property: bragi::PropertyId, _value: u32) -> Result<bool, k100::DeviceError> {
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

    struct Rig {
        bridge: SdkBridge,
        controller: Controller,
        replies: Receiver<Vec<u8>>,
        config: ConfigDir,
        enabled: Arc<AtomicBool>,
        _temp: tempfile::TempDir,
    }

    impl Rig {
        fn send(&mut self, client: ClientId, payload: &[u8]) {
            self.bridge.handle(SdkCommand::Message { client, payload: payload.into() }, &mut self.controller);
        }

        fn reply(&self) -> Value {
            serde_json::from_slice(&self.replies.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap()
        }

        fn attach(&mut self) {
            self.controller.attach(Box::new(NullLink), link_info(), Duration::ZERO);
        }
    }

    fn link_info() -> LinkInfo {
        LinkInfo {
            model: "K100".into(),
            firmware: "1.15.61".into(),
            max_polling_rate: None,
            serial: "SERIAL".into(),
            layout: 1,
        }
    }

    fn sdk_settings(sdk: SdkSettings) -> Settings {
        Settings { sdk: SdkSettings { device_id: Some(DEVICE.into()), ..sdk }, ..Settings::default() }
    }

    fn rig_with(settings: Settings, client: ClientId, exe: &str, queue: usize) -> Rig {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        config.save_settings(&settings).unwrap();
        let enabled = Arc::new(AtomicBool::new(settings.sdk.enabled));
        let mut controller = Controller::new(settings, vec![], vec![], Box::new(DiscardOutput));
        controller.attach(Box::new(NullLink), link_info(), Duration::ZERO);
        let mut bridge = SdkBridge::new(sdk::ServerConfig::default(), config.sdk_trace_path(), enabled.clone());
        let (outbox, replies) = sync_channel(queue);
        bridge.handle(SdkCommand::Connected { client, exe: exe.into(), outbox }, &mut controller);
        Rig { bridge, controller, replies, config, enabled, _temp: temp }
    }

    fn bridge_with_client_queue(client: ClientId, queue: usize) -> Rig {
        rig_with(sdk_settings(SdkSettings::default()), client, "game.exe", queue)
    }

    fn bridge_with_client(client: ClientId) -> Rig {
        bridge_with_client_queue(client, OUTBOX_QUEUE)
    }

    fn bridge_with_config_and_client(client: ClientId, exe: &str) -> Rig {
        rig_with(sdk_settings(SdkSettings::default()), client, exe, OUTBOX_QUEUE)
    }

    fn event_id(reply: &Value) -> Option<i64> {
        (reply["method"] == 17).then(|| reply["result"]["id"].as_i64()).flatten()
    }

    fn subscribed(client: ClientId) -> Rig {
        let mut rig = bridge_with_client(client);
        rig.send(client, HANDSHAKE);
        rig.send(client, SUBSCRIBE);
        assert_eq!(rig.reply()["result"]["state"], 6);
        assert_eq!(rig.reply()["method"], 14);
        let connected = rig.reply();
        assert_eq!(event_id(&connected), Some(DEVICE_EVENT));
        assert_eq!(connected["result"]["deviceConnectionStatusChangedEvent"]["isConnected"], true);
        rig
    }

    #[test]
    fn a_client_message_is_answered_through_its_outbox() {
        let mut rig = bridge_with_client(1);
        rig.send(1, HANDSHAKE);
        let reply: Value = serde_json::from_slice(&rig.replies.try_recv().unwrap()).unwrap();
        assert_eq!(reply["result"]["state"], 6);
        assert_eq!(rig.controller.state().sdk.clients[0].exe, "game.exe");
    }

    #[test]
    fn a_full_outbox_drops_the_client() {
        let mut rig = bridge_with_client_queue(1, 1);
        for _ in 0..2 {
            rig.send(1, HANDSHAKE);
        }
        assert!(rig.controller.state().sdk.clients.is_empty());
        assert!(rig.replies.try_recv().is_ok());
        assert_eq!(rig.replies.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn blocking_an_exe_closes_its_connection_and_is_saved() {
        let mut rig = bridge_with_config_and_client(1, "StreamDeck.exe");
        rig.send(1, HANDSHAKE);
        let request = ipc::Request::SdkBlock { exe: "streamdeck.exe".into(), blocked: true };
        assert_eq!(rig.controller.handle(request, &rig.config), ipc::Reply::Done);
        rig.bridge.sync(&mut rig.controller);
        assert!(rig.replies.recv_timeout(Duration::from_secs(1)).is_ok(), "the handshake reply was queued first");
        assert!(matches!(rig.replies.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Disconnected)));
        assert_eq!(rig.config.load_settings().unwrap().sdk.blocked, ["streamdeck.exe"]);
        assert!(rig.controller.state().sdk.clients.is_empty());
    }

    #[test]
    fn subscribing_announces_the_attached_keyboard() {
        subscribed(1);
    }

    #[test]
    fn g_key_input_reaches_subscribed_clients() {
        let mut rig = subscribed(1);
        rig.bridge.input(&InputEvent::KeyDown(led::G1), &mut rig.controller);
        rig.bridge.input(&InputEvent::KeyDown(27), &mut rig.controller);
        rig.bridge.input(&InputEvent::KeyUp(led::G1), &mut rig.controller);
        let pressed = rig.reply();
        assert_eq!(event_id(&pressed), Some(KEY_EVENT));
        assert_eq!(pressed["result"]["keyEvent"]["keyId"], 1);
        assert_eq!(pressed["result"]["keyEvent"]["isPressed"], true);
        assert_eq!(rig.reply()["result"]["keyEvent"]["isPressed"], false);
        assert!(rig.replies.try_recv().is_err(), "only G-keys are reported");
    }

    #[test]
    fn subscribers_hear_the_keyboard_go_and_come_back() {
        let mut rig = subscribed(1);
        rig.controller.set_device_state(ipc::DeviceState::Disconnected);
        rig.bridge.sync(&mut rig.controller);
        let gone = rig.reply();
        assert_eq!(event_id(&gone), Some(DEVICE_EVENT));
        assert_eq!(gone["result"]["deviceConnectionStatusChangedEvent"]["isConnected"], false);
        rig.attach();
        rig.bridge.sync(&mut rig.controller);
        assert_eq!(rig.reply()["result"]["deviceConnectionStatusChangedEvent"]["isConnected"], true);
        rig.bridge.sync(&mut rig.controller);
        assert!(rig.replies.try_recv().is_err(), "nothing changed since");
    }

    #[test]
    fn a_double_disconnect_and_messages_from_unknown_clients_are_harmless() {
        let mut rig = bridge_with_client(1);
        rig.bridge.handle(SdkCommand::Disconnected { client: 1 }, &mut rig.controller);
        rig.bridge.handle(SdkCommand::Disconnected { client: 1 }, &mut rig.controller);
        rig.send(1, HANDSHAKE);
        rig.send(9, HANDSHAKE);
        assert!(rig.controller.state().sdk.clients.is_empty());
        assert!(matches!(rig.replies.recv_timeout(Duration::from_millis(100)), Err(RecvTimeoutError::Disconnected)));
    }

    #[test]
    fn the_listener_status_shows_in_the_daemon_state() {
        let mut rig = bridge_with_client(1);
        rig.bridge.handle(SdkCommand::Status(SdkStatus::InUse), &mut rig.controller);
        assert_eq!(rig.controller.state().sdk.status, SdkStatus::InUse);
        rig.bridge.handle(SdkCommand::Status(SdkStatus::Listening), &mut rig.controller);
        assert_eq!(rig.controller.state().sdk.status, SdkStatus::Listening);
        let off = sdk_settings(SdkSettings { enabled: false, ..SdkSettings::default() });
        rig.controller.reload_settings(Ok(off));
        assert_eq!(rig.controller.state().sdk.status, SdkStatus::Off);
    }

    #[test]
    fn disabling_the_sdk_closes_the_pipe_gate_and_its_clients() {
        let mut rig = bridge_with_client(1);
        rig.send(1, HANDSHAKE);
        rig.bridge.sync(&mut rig.controller);
        assert!(rig.enabled.load(Ordering::SeqCst));
        rig.controller.reload_settings(Ok(sdk_settings(SdkSettings { enabled: false, ..SdkSettings::default() })));
        rig.bridge.sync(&mut rig.controller);
        assert!(!rig.enabled.load(Ordering::SeqCst));
        assert!(rig.replies.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(matches!(rig.replies.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Disconnected)));
        rig.controller.reload_settings(Ok(sdk_settings(SdkSettings::default())));
        rig.bridge.sync(&mut rig.controller);
        assert!(rig.enabled.load(Ordering::SeqCst));
    }

    #[test]
    fn an_sdk_state_change_is_announced_once() {
        let mut rig = bridge_with_client(1);
        let sdk_events = |rig: &mut Rig| -> Vec<ipc::Event> {
            let events = rig.controller.take_events();
            events.into_iter().filter(|event| matches!(event, ipc::Event::Sdk { .. })).collect()
        };
        rig.controller.tick(Duration::from_millis(16), &[], &rig.config);
        sdk_events(&mut rig);
        rig.controller.tick(Duration::from_millis(32), &[], &rig.config);
        assert_eq!(sdk_events(&mut rig), []);
        rig.send(1, HANDSHAKE);
        rig.bridge.handle(SdkCommand::Status(SdkStatus::Listening), &mut rig.controller);
        rig.controller.tick(Duration::from_millis(48), &[], &rig.config);
        rig.controller.tick(Duration::from_millis(64), &[], &rig.config);
        let events = sdk_events(&mut rig);
        assert_eq!(events.len(), 1, "{events:?}");
        let ipc::Event::Sdk { state } = &events[0] else { unreachable!() };
        assert_eq!((&state.status, state.clients.len()), (&SdkStatus::Listening, 1));
    }

    #[test]
    fn the_trace_file_records_messages_both_ways_when_enabled() {
        let traced = sdk_settings(SdkSettings { trace: true, ..SdkSettings::default() });
        let mut rig = rig_with(traced, 1, "game.exe", OUTBOX_QUEUE);
        rig.send(1, HANDSHAKE);
        rig.send(1, b"not json");
        let text = std::fs::read_to_string(rig.config.sdk_trace_path()).unwrap();
        let lines: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!((lines[0]["direction"].as_str(), lines[1]["direction"].as_str()), (Some("in"), Some("out")));
        assert_eq!((lines[0]["client"].as_u64(), lines[0]["exe"].as_str()), (Some(1), Some("game.exe")));
        assert_eq!(lines[0]["message"]["method"], 0);
        assert_eq!(lines[1]["message"]["result"]["state"], 6);
        assert!(lines[0]["at_ms"].as_u64().is_some());
        assert_eq!(lines[2]["message"], serde_json::json!({"bytes": 8}));
    }

    #[test]
    fn nothing_is_traced_while_tracing_is_off() {
        let mut rig = bridge_with_client(1);
        rig.send(1, HANDSHAKE);
        assert!(!rig.config.sdk_trace_path().exists());
    }

    #[test]
    fn device_ids_look_like_guids_and_differ() {
        let id = new_device_id();
        assert_eq!(id.len(), 38, "{id}");
        assert!(id.starts_with('{') && id.ends_with('}'), "{id}");
        for (index, character) in id.char_indices().skip(1).take(36) {
            if [9, 14, 19, 24].contains(&index) {
                assert_eq!(character, '-', "{id}");
            } else {
                assert!(matches!(character, '0'..='9' | 'a'..='f'), "{id}");
            }
        }
        assert_ne!(new_device_id(), new_device_id());
    }

    #[test]
    fn device_ids_are_rfc_4122_version_4() {
        for _ in 0..64 {
            let id = new_device_id();
            assert_eq!(id.as_bytes()[15], b'4', "{id}");
            assert!(matches!(id.as_bytes()[20], b'8' | b'9' | b'a' | b'b'), "{id}");
        }
    }

    #[test]
    fn a_missing_device_id_is_generated_once_and_kept() {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let first = with_device_id(&config, Settings::default());
        let id = first.sdk.device_id.clone().unwrap();
        assert_eq!(config.load_settings().unwrap().sdk.device_id.as_deref(), Some(id.as_str()));
        let second = with_device_id(&config, config.load_settings().unwrap());
        assert_eq!(second.sdk.device_id, Some(id));
        let explicit = sdk_settings(SdkSettings::default());
        assert_eq!(with_device_id(&config, explicit.clone()), explicit);
    }
}
