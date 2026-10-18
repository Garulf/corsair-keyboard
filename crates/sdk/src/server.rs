use crate::device::{
    DeviceSnapshot, device_info, led_for, leds, luid_for_key_name, macro_key, macro_key_led, property_flags,
    read_property,
};
use crate::host::{ClientSummary, SdkHost};
use crate::ids::{AccessLevel, ErrorCode, MacroKey, Method, STATE_CONNECTED, STATE_REFUSED, device_type};
use crate::message::{
    Envelope, LedColor, Request, RequestError, device_event, key_event, number, parse_envelope, parse_request, reply,
};
use engine::{ClientLayer, ClientLighting};
use k100::Rgb;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub type ClientId = u64;

pub const SERVER_VERSION: &str = "4.0.19";
pub const HOST_VERSION: &str = "5.52.93";
pub const DEFAULT_PRIORITY: u8 = 128;

const UNSET_COLOR: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub server_version: String,
    pub host_version: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { server_version: SERVER_VERSION.into(), host_version: HOST_VERSION.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Send { client: ClientId, payload: Vec<u8> },
    Close { client: ClientId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    AwaitingHandshake,
    Connected,
    Refused,
}

struct Session {
    id: ClientId,
    exe: String,
    stage: Stage,
    access: u8,
    priority: u8,
    colors: BTreeMap<u8, (Rgb, u8)>,
    intercepted: BTreeSet<MacroKey>,
    subscribed: bool,
}

impl Session {
    fn new(id: ClientId, exe: String) -> Self {
        Self {
            id,
            exe,
            stage: Stage::AwaitingHandshake,
            access: AccessLevel::Shared.bits(),
            priority: DEFAULT_PRIORITY,
            colors: BTreeMap::new(),
            intercepted: BTreeSet::new(),
            subscribed: false,
        }
    }

    fn access(&self) -> AccessLevel {
        AccessLevel::from_bits(self.access)
    }

    fn holds_lighting(&self) -> bool {
        self.access().lighting()
    }

    fn holds_keys(&self) -> bool {
        self.access().keys()
    }

    fn listening(&self) -> bool {
        self.stage == Stage::Connected && self.subscribed
    }

    fn set_access(&mut self, bits: u8) {
        self.access = bits;
        if !self.holds_keys() {
            self.intercepted.clear();
        }
    }

    fn give_up(&mut self, bits: u8) {
        self.set_access(self.access & !bits);
    }

    fn release(&mut self) {
        self.set_access(AccessLevel::Shared.bits());
        self.colors.clear();
    }

    fn paint(&mut self, colors: &[LedColor]) {
        for color in colors {
            if let Some(led) = led_for(color.luid) {
                self.colors.insert(led, (color.rgb, color.alpha));
            }
        }
    }

    fn layer(&self) -> ClientLayer {
        let colors = self.colors.iter().map(|(&led, &(rgb, alpha))| (led, rgb, alpha)).collect();
        ClientLayer { priority: self.priority, colors }
    }

    fn summary(&self) -> ClientSummary {
        ClientSummary {
            id: self.id,
            exe: self.exe.clone(),
            refused: self.stage == Stage::Refused,
            access: self.access(),
            priority: self.priority,
            intercepted: self.intercepted.iter().copied().collect(),
            subscribed: self.subscribed,
        }
    }
}

#[derive(Default)]
struct Published {
    lighting: ClientLighting,
    intercepted: Vec<u8>,
    clients: Vec<ClientSummary>,
}

type Answer = Result<Value, ErrorCode>;

pub struct Server {
    config: ServerConfig,
    sessions: Vec<Session>,
    noted_exes: HashSet<String>,
    device: Option<String>,
    held_keys: BTreeMap<u8, Vec<ClientId>>,
    published: Published,
}

impl Server {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            sessions: Vec::new(),
            noted_exes: HashSet::new(),
            device: None,
            held_keys: BTreeMap::new(),
            published: Published::default(),
        }
    }

    pub fn connect(&mut self, client: ClientId, exe: String, host: &mut dyn SdkHost) {
        if self.note_first_connection(&exe) {
            eprintln!("k100d: iCUE SDK client {exe} connected");
        }
        self.remove(client);
        self.sessions.push(Session::new(client, exe));
        self.publish(host);
    }

    pub fn receive(&mut self, client: ClientId, payload: &[u8], host: &mut dyn SdkHost) -> Vec<Output> {
        let mut outputs = self.follow_device(host);
        if self.session(client).is_none() {
            return outputs;
        }
        match parse_envelope(payload) {
            Ok(envelope) => {
                let (error, result) = match self.answer(client, &envelope, host) {
                    Ok(result) => (ErrorCode::Success, result),
                    Err(error) => (error, json!({})),
                };
                outputs.push(Output::Send { client, payload: reply(envelope.method, envelope.code, error, result) });
                if error == ErrorCode::Success && envelope.method == Method::Subscribe.id() {
                    outputs
                        .extend(self.device.iter().map(|id| Output::Send { client, payload: device_event(id, true) }));
                }
            }
            Err(_) => {
                self.remove(client);
                outputs.push(Output::Close { client });
            }
        }
        self.publish(host);
        outputs
    }

    pub fn disconnect(&mut self, client: ClientId, host: &mut dyn SdkHost) {
        self.remove(client);
        self.publish(host);
    }

    pub fn key(&mut self, led: u8, pressed: bool, host: &mut dyn SdkHost) -> Vec<Output> {
        let mut outputs = self.follow_device(host);
        let (Some(key), Some(device)) = (macro_key(led), self.device.clone()) else {
            return outputs;
        };
        let recipients = if pressed { self.press(led, key) } else { self.release_key(led) };
        let payload = key_event(&device, key, pressed);
        outputs.extend(recipients.into_iter().map(|client| Output::Send { client, payload: payload.clone() }));
        outputs
    }

    pub fn sync(&mut self, host: &mut dyn SdkHost) -> Vec<Output> {
        let policy = host.policy();
        let closing: Vec<ClientId> = self
            .sessions
            .iter()
            .filter(|session| session.stage == Stage::Connected && !policy.allows(&session.exe))
            .map(|session| session.id)
            .collect();
        for &client in &closing {
            self.remove(client);
        }
        if !policy.exclusive_control {
            for session in &mut self.sessions {
                session.give_up(AccessLevel::Exclusive.bits());
            }
        }
        let mut outputs: Vec<Output> = closing.into_iter().map(|client| Output::Close { client }).collect();
        outputs.extend(self.follow_device(host));
        self.publish(host);
        outputs
    }

    pub fn clients(&self) -> Vec<ClientSummary> {
        self.sessions.iter().map(Session::summary).collect()
    }

    fn note_first_connection(&mut self, exe: &str) -> bool {
        self.noted_exes.insert(exe.to_ascii_lowercase())
    }

    fn session(&self, client: ClientId) -> Option<&Session> {
        self.sessions.iter().find(|session| session.id == client)
    }

    fn served(&mut self, client: ClientId) -> &mut Session {
        self.sessions.iter_mut().find(|session| session.id == client).expect("only connected clients are served")
    }

    fn remove(&mut self, client: ClientId) {
        self.sessions.retain(|session| session.id != client);
        for recipients in self.held_keys.values_mut() {
            recipients.retain(|&recipient| recipient != client);
        }
    }

    fn listeners(&self) -> Vec<ClientId> {
        self.sessions.iter().filter(|session| session.listening()).map(|session| session.id).collect()
    }

    fn broadcast(&self, payload: &[u8]) -> Vec<Output> {
        self.listeners().into_iter().map(|client| Output::Send { client, payload: payload.to_vec() }).collect()
    }

    fn follow_device(&mut self, host: &dyn SdkHost) -> Vec<Output> {
        let current = host.device().map(|device| device.id);
        if current == self.device {
            return Vec::new();
        }
        let mut outputs = match self.device.take() {
            Some(previous) => {
                let mut released = self.release_held_keys(&previous);
                released.extend(self.broadcast(&device_event(&previous, false)));
                released
            }
            None => Vec::new(),
        };
        if let Some(id) = current.as_deref() {
            outputs.extend(self.broadcast(&device_event(id, true)));
        }
        self.device = current;
        outputs
    }

    fn release_held_keys(&mut self, device: &str) -> Vec<Output> {
        let held = std::mem::take(&mut self.held_keys);
        held.into_iter()
            .filter_map(|(led, recipients)| macro_key(led).map(|key| (key_event(device, key, false), recipients)))
            .flat_map(|(payload, recipients)| {
                recipients
                    .into_iter()
                    .filter(|&client| self.session(client).is_some_and(Session::listening))
                    .map(move |client| Output::Send { client, payload: payload.clone() })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn press(&mut self, led: u8, key: MacroKey) -> Vec<ClientId> {
        let interceptor =
            self.sessions.iter().find(|session| session.holds_keys() && session.intercepted.contains(&key));
        let recipients = match interceptor {
            Some(holder) => holder.listening().then_some(holder.id).into_iter().collect(),
            None => self.listeners(),
        };
        let held = self.held_keys.entry(led).or_default();
        for &client in &recipients {
            if !held.contains(&client) {
                held.push(client);
            }
        }
        recipients
    }

    fn release_key(&mut self, led: u8) -> Vec<ClientId> {
        let held = self.held_keys.remove(&led).unwrap_or_default();
        held.into_iter().filter(|&client| self.session(client).is_some_and(Session::listening)).collect()
    }

    fn answer(&mut self, client: ClientId, envelope: &Envelope, host: &mut dyn SdkHost) -> Answer {
        let stage = self.session(client).map(|session| session.stage);
        let handshake = envelope.method == Method::Handshake.id();
        match stage {
            Some(Stage::Connected) => {}
            Some(Stage::AwaitingHandshake) if handshake => {}
            _ => return Err(ErrorCode::NotConnected),
        }
        let request = parse_request(envelope).map_err(|error| match error {
            RequestError::UnknownMethod(_) => ErrorCode::IncompatibleProtocol,
            RequestError::BadParams(_) => ErrorCode::InvalidArguments,
        })?;
        self.serve(client, request, host)
    }

    fn serve(&mut self, client: ClientId, request: Request, host: &mut dyn SdkHost) -> Answer {
        match request {
            Request::Handshake { .. } => Ok(self.handshake(client, host)),
            Request::GetDevices { type_mask, size_max } => {
                let wanted = type_mask & device_type::KEYBOARD != 0 && size_max >= 1;
                let infos: Vec<Value> = host.device().filter(|_| wanted).iter().map(device_info).collect();
                Ok(json!({"deviceInfos": infos}))
            }
            Request::GetDeviceInfo { device_id } => Ok(device_info(&attached(host, &device_id)?)),
            Request::GetPropertyInfo { device_id, property, .. } => {
                let device = attached(host, &device_id)?;
                let flags = property_flags(property)?;
                let data_type = read_property(&device, property)?.data_type();
                Ok(json!({"dataType": data_type.id(), "flags": flags}))
            }
            Request::ReadProperty { device_id, property, .. } => {
                Ok(read_property(&attached(host, &device_id)?, property)?.to_json())
            }
            Request::WriteProperty { device_id, .. } => {
                attached(host, &device_id)?;
                Err(ErrorCode::InvalidOperation)
            }
            Request::LuidForKeyName { device_id, key } => {
                attached(host, &device_id)?;
                let luid = luid_for_key_name(key).ok_or(ErrorCode::InvalidArguments)?;
                Ok(json!({"ledLuid": luid.0}))
            }
            Request::GetLedPositions { device_id, size_max } => {
                attached(host, &device_id)?;
                let count = usize::try_from(size_max).unwrap_or(0);
                let positions: Vec<Value> = leds()
                    .iter()
                    .take(count)
                    .map(|led| json!({"cx": number(led.cx), "cy": number(led.cy), "id": led.luid.0}))
                    .collect();
                Ok(json!({"ledPositions": positions}))
            }
            Request::GetLedColors { device_id, luids } => {
                attached(host, &device_id)?;
                let session = self.served(client);
                let colors: Vec<Value> = luids
                    .into_iter()
                    .filter_map(|luid| {
                        let led = led_for(luid)?;
                        let value = session
                            .colors
                            .get(&led)
                            .map_or(UNSET_COLOR, |&(rgb, alpha)| LedColor { luid, rgb, alpha }.value());
                        Some(json!({"id": luid.0, "value": value}))
                    })
                    .collect();
                Ok(json!({"ledColors": colors}))
            }
            Request::SetLayerPriority { priority } => {
                self.served(client).priority = u8::try_from(priority).map_err(|_| ErrorCode::InvalidArguments)?;
                Ok(json!({}))
            }
            Request::SetLedColors { device_id, colors } => {
                attached(host, &device_id)?;
                self.check_lighting_control(client)?;
                self.served(client).paint(&colors);
                Ok(json!({}))
            }
            Request::FlushLedColors { devices } => {
                for (device_id, _) in &devices {
                    attached(host, device_id)?;
                }
                self.check_lighting_control(client)?;
                for (_, colors) in &devices {
                    self.served(client).paint(colors);
                }
                Ok(json!({}))
            }
            Request::RequestControl { device_id, level } => {
                attached(host, &device_id)?;
                let level = AccessLevel::from_id(level).ok_or(ErrorCode::InvalidArguments)?;
                if level != AccessLevel::Shared && !host.policy().exclusive_control {
                    return Err(ErrorCode::NotAllowed);
                }
                self.take_control(client, level);
                Ok(json!({}))
            }
            Request::ReleaseControl { device_id } => {
                attached(host, &device_id)?;
                self.served(client).release();
                Ok(json!({}))
            }
            Request::Subscribe => {
                self.served(client).subscribed = true;
                Ok(json!({}))
            }
            Request::Unsubscribe => {
                self.served(client).subscribed = false;
                Ok(json!({}))
            }
            Request::ConfigureKeyEvent { device_id, key, intercepted } => {
                attached(host, &device_id)?;
                let key = u8::try_from(key)
                    .ok()
                    .map(MacroKey)
                    .filter(|&key| macro_key_led(key).is_some())
                    .ok_or(ErrorCode::InvalidArguments)?;
                let session = self.served(client);
                if !session.holds_keys() {
                    return Err(ErrorCode::NoControl);
                }
                if intercepted {
                    session.intercepted.insert(key);
                } else {
                    session.intercepted.remove(&key);
                }
                Ok(json!({}))
            }
        }
    }

    fn handshake(&mut self, client: ClientId, host: &dyn SdkHost) -> Value {
        let allowed = host.policy().allows(&self.served(client).exe);
        let session = self.served(client);
        let state = if allowed {
            session.stage = Stage::Connected;
            STATE_CONNECTED
        } else {
            session.release();
            session.subscribed = false;
            session.stage = Stage::Refused;
            STATE_REFUSED
        };
        json!({
            "serverHostVersion": self.config.host_version,
            "serverVersion": self.config.server_version,
            "state": state,
        })
    }

    fn check_lighting_control(&self, client: ClientId) -> Result<(), ErrorCode> {
        let held_elsewhere = self.sessions.iter().any(|session| session.id != client && session.holds_lighting());
        if held_elsewhere { Err(ErrorCode::NoControl) } else { Ok(()) }
    }

    fn take_control(&mut self, client: ClientId, level: AccessLevel) {
        for session in &mut self.sessions {
            if session.id == client {
                session.set_access(level.bits());
            } else {
                session.give_up(level.bits());
            }
        }
    }

    fn lighting(&self) -> ClientLighting {
        if let Some(holder) = self.sessions.iter().find(|session| session.holds_lighting()) {
            return ClientLighting::Exclusive(holder.layer());
        }
        let mut painted: Vec<&Session> = self.sessions.iter().filter(|session| !session.colors.is_empty()).collect();
        painted.sort_by_key(|session| session.priority);
        ClientLighting::Shared(painted.into_iter().map(Session::layer).collect())
    }

    fn intercepted_leds(&self) -> Vec<u8> {
        let leds: BTreeSet<u8> = self
            .sessions
            .iter()
            .filter(|session| session.holds_keys())
            .flat_map(|session| session.intercepted.iter().filter_map(|&key| macro_key_led(key)))
            .collect();
        leds.into_iter().collect()
    }

    fn publish(&mut self, host: &mut dyn SdkHost) {
        let lighting = self.lighting();
        if lighting != self.published.lighting {
            self.published.lighting = lighting.clone();
            host.set_lighting(lighting);
        }
        let intercepted = self.intercepted_leds();
        if intercepted != self.published.intercepted {
            self.published.intercepted = intercepted.clone();
            host.set_intercepted(intercepted);
        }
        let clients = self.clients();
        if clients != self.published.clients {
            self.published.clients = clients.clone();
            host.clients_changed(clients);
        }
    }
}

fn attached(host: &dyn SdkHost, device_id: &str) -> Result<DeviceSnapshot, ErrorCode> {
    host.device().filter(|device| device.id == device_id).ok_or(ErrorCode::DeviceNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Policy;
    use crate::ids::{AccessLevel, MacroKey};
    use crate::message::{device_event, key_event, number};
    use crate::testing::*;
    use crate::transcript::{Direction, Transcript, differences, normalize};
    use crate::{device_info, leds};
    use engine::{ClientLayer, ClientLighting};
    use k100::Rgb;
    use serde_json::{Value, json};

    const SUBSCRIBE: &str = r#"{"method":14,"code":5}"#;
    const RED: u32 = 0xFFFF_0000;
    const BLUE: u32 = 0xFF00_00FF;

    fn server_and_host() -> (Server, FakeHost) {
        (Server::new(ServerConfig::default()), FakeHost::k100())
    }

    fn control(code: i64, level: i64) -> String {
        format!(r#"{{"method":12,"code":{code},"params":{{"deviceId":"{DEVICE}","accessLevel":{level}}}}}"#)
    }

    fn release(code: i64) -> String {
        format!(r#"{{"method":13,"code":{code},"params":{{"deviceId":"{DEVICE}"}}}}"#)
    }

    fn intercept(code: i64, key: i64, intercepted: bool) -> String {
        format!(
            r#"{{"method":16,"code":{code},"params":{{"deviceId":"{DEVICE}","keyId":{key},"isIntercepted":{intercepted}}}}}"#
        )
    }

    fn priority(code: i64, priority: i64) -> String {
        format!(r#"{{"method":9,"code":{code},"params":{{"priority":{priority}}}}}"#)
    }

    fn error(replies: &[Value]) -> i64 {
        replies[0]["error"].as_i64().expect("an error code")
    }

    fn summary(host: &FakeHost, client: ClientId) -> &ClientSummary {
        host.clients.iter().find(|summary| summary.id == client).expect("a client summary")
    }

    fn shared_layers(host: &FakeHost) -> &[ClientLayer] {
        let ClientLighting::Shared(layers) = &host.lighting else { panic!("expected shared lighting") };
        layers
    }

    fn get_devices(server: &mut Server, host: &mut FakeHost, mask: i64, size_max: i64) -> Value {
        let request = format!(r#"{{"method":1,"code":1,"params":{{"deviceTypeMask":{mask},"sizeMax":{size_max}}}}}"#);
        exchange(server, host, 1, &request)[0]["result"]["deviceInfos"].clone()
    }

    fn event_payloads(outputs: &[Output], client: ClientId) -> Vec<Vec<u8>> {
        outputs
            .iter()
            .filter_map(|output| match output {
                Output::Send { client: to, payload } if *to == client => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }

    type Exception = (&'static str, fn(&mut Transcript));

    const ICUE_EXCEPTIONS: [Exception; 1] =
        [("the other Corsair devices iCUE listed: k100d serves the K100 only", drop_other_devices)];

    fn drop_other_devices(capture: &mut Transcript) {
        let other = |value: &Value| value.as_str().is_some_and(|id| id != DEVICE);
        capture
            .entries
            .retain(|entry| !other(&entry.message["result"]["deviceConnectionStatusChangedEvent"]["deviceId"]));
        for entry in &mut capture.entries {
            if let Some(Value::Array(infos)) = entry.message["result"].get_mut("deviceInfos") {
                infos.retain(|info| !other(&info["id"]));
            }
        }
    }

    fn events(transcript: &Transcript) -> Vec<Value> {
        transcript
            .entries
            .iter()
            .filter(|entry| entry.direction == Direction::Received && entry.message["method"] == 17)
            .map(|entry| normalize(&entry.message))
            .collect()
    }

    fn unnormalized_replies(transcript: &Transcript, method: i64) -> Vec<&Value> {
        transcript
            .entries
            .iter()
            .filter(|entry| entry.direction == Direction::Received && entry.message["method"] == method)
            .map(|entry| &entry.message)
            .collect()
    }

    #[test]
    fn the_server_answers_the_icue_capture_like_icue() {
        let mut capture: Transcript = serde_json::from_str(include_str!("../fixtures/icue-k100-capture.json")).unwrap();
        for (_, exception) in ICUE_EXCEPTIONS {
            exception(&mut capture);
        }
        let ours = replay(&capture, FakeHost::k100());
        assert_eq!(differences(&capture, &ours), Vec::<String>::new());
        assert_eq!(events(&ours), events(&capture));
        for method in [Method::Handshake, Method::GetLedColors] {
            assert_eq!(unnormalized_replies(&ours, method.id()), unnormalized_replies(&capture, method.id()));
        }
    }

    #[test]
    fn the_handshake_connects_with_the_server_versions() {
        let (mut server, mut host) = server_and_host();
        server.connect(1, "game.exe".into(), &mut host);
        assert_eq!(
            exchange(&mut server, &mut host, 1, HANDSHAKE),
            [json!({"code": -1, "error": 0, "method": 0,
                    "result": {"serverHostVersion": HOST_VERSION, "serverVersion": "4.0.19", "state": 6}})]
        );
    }

    #[test]
    fn a_blocked_exe_is_refused_and_then_told_it_is_not_connected() {
        let mut host = FakeHost {
            policy: Policy { blocked: vec!["streamdeck.EXE".into()], ..Policy::default() },
            ..FakeHost::k100()
        };
        let mut server = Server::new(ServerConfig::default());
        server.connect(1, "StreamDeck.exe".into(), &mut host);
        assert_eq!(exchange(&mut server, &mut host, 1, HANDSHAKE)[0]["result"]["state"], 4);
        let later =
            exchange(&mut server, &mut host, 1, r#"{"method":1,"code":1,"params":{"deviceTypeMask":1,"sizeMax":64}}"#);
        assert_eq!(later, [json!({"code": 1, "error": 1, "method": 1, "result": {}})]);
        assert!(host.clients[0].refused);
        assert_eq!(
            exchange(&mut server, &mut host, 1, HANDSHAKE),
            [json!({"code": -1, "error": 1, "method": 0, "result": {}})]
        );
    }

    #[test]
    fn a_disabled_sdk_refuses_every_handshake() {
        let mut host = FakeHost { policy: Policy { enabled: false, ..Policy::default() }, ..FakeHost::k100() };
        let mut server = Server::new(ServerConfig::default());
        for (client, exe) in [(1, "game.exe"), (2, "StreamDeck.exe")] {
            server.connect(client, exe.into(), &mut host);
            assert_eq!(exchange(&mut server, &mut host, client, HANDSHAKE)[0]["result"]["state"], 4, "{exe}");
        }
        assert!(host.clients.iter().all(|summary| summary.refused));
    }

    #[test]
    fn requests_before_the_handshake_are_not_connected() {
        let (mut server, mut host) = server_and_host();
        server.connect(1, "game.exe".into(), &mut host);
        let reply =
            exchange(&mut server, &mut host, 1, r#"{"method":1,"code":3,"params":{"deviceTypeMask":1,"sizeMax":64}}"#);
        assert_eq!(reply, [json!({"code": 3, "error": 1, "method": 1, "result": {}})]);
        assert_eq!(exchange(&mut server, &mut host, 1, HANDSHAKE)[0]["result"]["state"], 6);
    }

    #[test]
    fn devices_list_the_k100_or_nothing() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        let k100 = json!([device_info(host.device.as_ref().unwrap())]);
        assert_eq!(get_devices(&mut server, &mut host, 1, 64), k100);
        assert_eq!(get_devices(&mut server, &mut host, -1, 64), k100);
        assert_eq!(get_devices(&mut server, &mut host, 4_294_967_295, 64), json!([]), "too big for an int");
        assert_eq!(get_devices(&mut server, &mut host, 2, 64), json!([]));
        assert_eq!(get_devices(&mut server, &mut host, 1, 0), json!([]));
        host.device = None;
        assert_eq!(get_devices(&mut server, &mut host, 1, 64), json!([]));
    }

    #[test]
    fn device_calls_with_an_unknown_id_are_device_not_found() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        let info = exchange(&mut server, &mut host, 1, r#"{"method":2,"code":4,"params":{"deviceId":"{nope}"}}"#);
        assert_eq!(info, [json!({"code": 4, "error": 6, "method": 2, "result": {}})]);
        let ok = exchange(
            &mut server,
            &mut host,
            1,
            &format!(r#"{{"method":2,"code":5,"params":{{"deviceId":"{DEVICE}"}}}}"#),
        );
        assert_eq!(ok[0]["result"], device_info(host.device.as_ref().unwrap()));
        host.device = None;
        let positions = format!(r#"{{"method":7,"code":6,"params":{{"deviceId":"{DEVICE}","sizeMax":512}}}}"#);
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &positions)), 6);
    }

    #[test]
    fn positions_and_luids_come_from_the_k100_model() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        let positions = |size_max: i64| {
            format!(r#"{{"method":7,"code":2,"params":{{"deviceId":"{DEVICE}","sizeMax":{size_max}}}}}"#)
        };
        let all = exchange(&mut server, &mut host, 1, &positions(512));
        let entries = all[0]["result"]["ledPositions"].as_array().unwrap().clone();
        assert_eq!(entries.len(), 173);
        assert_eq!(entries[0], json!({"cx": number(leds()[0].cx), "cy": number(leds()[0].cy), "id": leds()[0].luid.0}));
        assert_eq!(entries[0], json!({"cx": 498, "cy": 71, "id": 131_099}));
        let ten = exchange(&mut server, &mut host, 1, &positions(10));
        assert_eq!(ten[0]["result"]["ledPositions"].as_array().unwrap().len(), 10);
        let luid = |key: &str| format!(r#"{{"method":6,"code":3,"params":{{"deviceId":"{DEVICE}","keyName":{key}}}}}"#);
        assert_eq!(exchange(&mut server, &mut host, 1, &luid("87"))[0]["result"], json!({"ledLuid": 30}));
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &luid("63"))), 4);
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &luid(r#""W""#))), 4, "names go as char codes");
    }

    #[test]
    fn properties_answer_info_and_values() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        let property = |method: i64, id: i64| {
            format!(r#"{{"method":{method},"code":7,"params":{{"deviceId":"{DEVICE}","propertyId":{id},"index":0}}}}"#)
        };
        assert_eq!(
            exchange(&mut server, &mut host, 1, &property(3, 8))[0]["result"],
            json!({"dataType": 17, "flags": 1})
        );
        assert_eq!(
            exchange(&mut server, &mut host, 1, &property(4, 8))[0]["result"],
            json!({"dataType": 17, "value": [1, 2, 3, 4, 5, 6]})
        );
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &property(4, 2))), 5);
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &property(3, 2))), 5);
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &property(5, 6))), 5);
    }

    #[test]
    fn shared_clients_stack_by_priority_with_the_default_at_128() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "a.exe");
        connected(&mut server, &mut host, 2, "b.exe");
        exchange(&mut server, &mut host, 1, &set_colors(2, &[(30, RED)]));
        exchange(&mut server, &mut host, 2, &priority(2, 50));
        exchange(&mut server, &mut host, 2, &set_colors(3, &[(30, BLUE)]));
        let layers = shared_layers(&host);
        assert_eq!(layers.iter().map(|layer| layer.priority).collect::<Vec<_>>(), [50, DEFAULT_PRIORITY]);
        assert_eq!(layers[0].colors, [(26, Rgb::new(0, 0, 255), 255)]);
        assert_eq!(layers[1].colors, [(26, Rgb::new(255, 0, 0), 255)]);
    }

    #[test]
    fn equal_priorities_stack_in_connection_order() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "a.exe");
        connected(&mut server, &mut host, 2, "b.exe");
        exchange(&mut server, &mut host, 2, &set_colors(1, &[(30, BLUE)]));
        exchange(&mut server, &mut host, 1, &set_colors(1, &[(30, RED)]));
        let layers = shared_layers(&host);
        assert_eq!(layers[0].colors, [(26, Rgb::new(255, 0, 0), 255)], "client 1 below");
        assert_eq!(layers[1].colors, [(26, Rgb::new(0, 0, 255), 255)], "client 2 on top");
    }

    #[test]
    fn priorities_outside_0_to_255_are_invalid_arguments() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "a.exe");
        for bad in [256, -1] {
            assert_eq!(error(&exchange(&mut server, &mut host, 1, &priority(1, bad))), 4, "{bad}");
        }
        assert_eq!(summary(&host, 1).priority, DEFAULT_PRIORITY);
        assert_eq!(
            exchange(&mut server, &mut host, 1, &priority(2, 255)),
            [json!({"code": 2, "error": 0, "method": 9, "result": {}})]
        );
        assert_eq!(summary(&host, 1).priority, 255);
    }

    #[test]
    fn flush_applies_colors_for_every_listed_device() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "a.exe");
        let flush = |device: &str| {
            json!({"method": 11, "code": 4, "params": {"devices": [
                {"deviceId": DEVICE, "ledColors": [{"id": 30, "value": RED}]},
                {"deviceId": device, "ledColors": [{"id": 42, "value": RED}]},
            ]}})
            .to_string()
        };
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &flush("{nope}"))), 6);
        assert_eq!(host.lighting, ClientLighting::default());
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &flush(DEVICE))), 0);
        let red = Rgb::new(255, 0, 0);
        assert_eq!(shared_layers(&host)[0].colors, [(4, red, 255), (26, red, 255)]);
    }

    #[test]
    fn unknown_luids_are_skipped_and_the_rest_apply() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "a.exe");
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &set_colors(1, &[(30, RED), (0x7FFF_0001, RED)]))), 0);
        assert_eq!(shared_layers(&host)[0].colors, [(26, Rgb::new(255, 0, 0), 255)]);
    }

    #[test]
    fn exclusive_lighting_replaces_the_stack_and_shuts_out_other_clients() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "a.exe");
        connected(&mut server, &mut host, 2, "b.exe");
        exchange(&mut server, &mut host, 1, &set_colors(1, &[(30, RED)]));
        let a_layer = shared_layers(&host)[0].clone();
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &control(1, 1))), 0);
        let b_layer = ClientLayer { priority: DEFAULT_PRIORITY, colors: vec![] };
        assert_eq!(host.lighting, ClientLighting::Exclusive(b_layer.clone()));
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &set_colors(2, &[(42, BLUE)]))), 2);
        assert_eq!(host.lighting, ClientLighting::Exclusive(b_layer));
        exchange(&mut server, &mut host, 2, &set_colors(2, &[(42, BLUE)]));
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &release(3))), 0);
        assert_eq!(host.lighting, ClientLighting::Shared(vec![a_layer]));
    }

    #[test]
    fn a_later_exclusive_request_takes_over() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 2, "b.exe");
        connected(&mut server, &mut host, 3, "c.exe");
        exchange(&mut server, &mut host, 2, &control(1, 1));
        exchange(&mut server, &mut host, 3, &control(1, 1));
        exchange(&mut server, &mut host, 3, &set_colors(2, &[(30, BLUE)]));
        let c_layer = ClientLayer { priority: DEFAULT_PRIORITY, colors: vec![(26, Rgb::new(0, 0, 255), 255)] };
        assert_eq!(host.lighting, ClientLighting::Exclusive(c_layer));
        assert_eq!(
            (summary(&host, 2).access, summary(&host, 3).access),
            (AccessLevel::Shared, AccessLevel::ExclusiveLighting)
        );
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &set_colors(3, &[(30, RED)]))), 2);
    }

    #[test]
    fn exclusive_capabilities_are_taken_one_at_a_time() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        connected(&mut server, &mut host, 2, "StreamDeck.exe");
        exchange(&mut server, &mut host, 1, &control(1, 1));
        exchange(&mut server, &mut host, 2, &control(1, 2));
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &intercept(2, 1, true))), 0);
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &set_colors(2, &[(30, RED)]))), 0);
        assert_eq!(
            (summary(&host, 1).access, summary(&host, 2).access),
            (AccessLevel::ExclusiveLighting, AccessLevel::ExclusiveKeys)
        );
        assert!(matches!(host.lighting, ClientLighting::Exclusive(ref layer) if !layer.colors.is_empty()));
        assert_eq!(host.intercepted, [131]);

        connected(&mut server, &mut host, 3, "other.exe");
        exchange(&mut server, &mut host, 3, &control(1, 2));
        assert_eq!(
            (summary(&host, 1).access, summary(&host, 2).access, summary(&host, 3).access),
            (AccessLevel::ExclusiveLighting, AccessLevel::Shared, AccessLevel::ExclusiveKeys)
        );
        assert!(summary(&host, 2).intercepted.is_empty());
        assert!(host.intercepted.is_empty());

        exchange(&mut server, &mut host, 3, &control(2, 3));
        assert_eq!((summary(&host, 1).access, summary(&host, 3).access), (AccessLevel::Shared, AccessLevel::Exclusive));
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &set_colors(3, &[(30, RED)]))), 2);
    }

    #[test]
    fn exclusive_requests_are_not_allowed_when_the_setting_is_off() {
        let mut host =
            FakeHost { policy: Policy { exclusive_control: false, ..Policy::default() }, ..FakeHost::k100() };
        let mut server = Server::new(ServerConfig::default());
        connected(&mut server, &mut host, 1, "game.exe");
        for level in [1, 2, 3] {
            assert_eq!(error(&exchange(&mut server, &mut host, 1, &control(1, level))), 7, "{level}");
        }
        assert_eq!(summary(&host, 1).access, AccessLevel::Shared);
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &control(2, 0))), 0);
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &control(3, 4))), 4);
    }

    #[test]
    fn turning_exclusive_control_off_demotes_the_holders() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        exchange(&mut server, &mut host, 1, &control(1, 3));
        exchange(&mut server, &mut host, 1, &intercept(2, 1, true));
        exchange(&mut server, &mut host, 1, &set_colors(3, &[(30, RED)]));
        host.policy.exclusive_control = false;
        assert!(server.sync(&mut host).is_empty());
        assert_eq!((summary(&host, 1).access, summary(&host, 1).intercepted.clone()), (AccessLevel::Shared, vec![]));
        assert!(host.intercepted.is_empty());
        assert_eq!(shared_layers(&host)[0].colors, [(26, Rgb::new(255, 0, 0), 255)]);
    }

    #[test]
    fn configuring_key_events_needs_exclusive_key_listening() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "shared.exe");
        connected(&mut server, &mut host, 2, "holder.exe");
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &intercept(1, 1, true))), 2);
        exchange(&mut server, &mut host, 2, &control(1, 2));
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &intercept(2, 7, true))), 4);
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &intercept(2, 0, true))), 4);
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &intercept(3, 6, true))), 0);
        assert_eq!(host.intercepted, [136]);
        assert_eq!(error(&exchange(&mut server, &mut host, 2, &intercept(4, 6, false))), 0);
        assert!(host.intercepted.is_empty());
    }

    #[test]
    fn intercepted_g_keys_go_only_to_the_exclusive_listener() {
        let (mut server, mut host) = server_and_host();
        for client in [1, 2] {
            connected(&mut server, &mut host, client, "x.exe");
            exchange(&mut server, &mut host, client, r#"{"method":14,"code":2}"#);
        }
        exchange(&mut server, &mut host, 2, &control(3, 2));
        assert_eq!(exchange(&mut server, &mut host, 2, &intercept(4, 1, true))[0]["error"], 0);
        assert_eq!(host.intercepted, [131]);
        let g1 = server.key(131, true, &mut host);
        assert_eq!(recipients(&g1), [2]);
        let Output::Send { payload, .. } = &g1[0] else { panic!("expected a send") };
        assert_eq!(
            serde_json::from_slice::<Value>(payload).unwrap(),
            json!({"code": -1, "error": 0, "method": 17,
                   "result": {"id": 2, "keyEvent": {"deviceId": DEVICE, "isPressed": true, "keyId": 1}}})
        );
        assert_eq!(recipients(&server.key(132, true, &mut host)), [1, 2]);
        assert!(server.key(41, true, &mut host).is_empty());
    }

    #[test]
    fn a_key_held_while_interception_changes_is_released_to_whoever_saw_it_pressed() {
        let (mut server, mut host) = server_and_host();
        for client in [1, 2] {
            connected(&mut server, &mut host, client, "x.exe");
            exchange(&mut server, &mut host, client, SUBSCRIBE);
        }
        assert_eq!(recipients(&server.key(131, true, &mut host)), [1, 2]);
        exchange(&mut server, &mut host, 2, &control(1, 2));
        exchange(&mut server, &mut host, 2, &intercept(2, 1, true));
        assert_eq!(recipients(&server.key(131, false, &mut host)), [1, 2]);

        assert_eq!(recipients(&server.key(131, true, &mut host)), [2]);
        exchange(&mut server, &mut host, 2, &release(3));
        assert_eq!(recipients(&server.key(131, false, &mut host)), [2]);
        assert!(server.key(131, false, &mut host).is_empty(), "a second release goes nowhere");
    }

    #[test]
    fn unplugging_releases_held_keys_before_the_disconnect_event() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "x.exe");
        exchange(&mut server, &mut host, 1, SUBSCRIBE);
        server.key(132, true, &mut host);
        host.device = None;
        let unplugged = server.sync(&mut host);
        assert_eq!(event_payloads(&unplugged, 1), [key_event(DEVICE, MacroKey(2), false), device_event(DEVICE, false)]);
        assert!(server.key(132, false, &mut host).is_empty());
    }

    #[test]
    fn unsubscribed_clients_get_no_events() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "x.exe");
        assert!(server.key(131, true, &mut host).is_empty(), "not subscribed yet");
        let connected_event = serde_json::from_slice::<Value>(&device_event(DEVICE, true)).unwrap();
        assert_eq!(
            exchange(&mut server, &mut host, 1, SUBSCRIBE),
            [json!({"code": 5, "error": 0, "method": 14, "result": {}}), connected_event]
        );
        assert!(summary(&host, 1).subscribed);
        exchange(&mut server, &mut host, 1, r#"{"method":15,"code":6}"#);
        assert!(!summary(&host, 1).subscribed);
        assert!(server.key(131, true, &mut host).is_empty());
    }

    #[test]
    fn unplugging_the_keyboard_tells_subscribers_and_replugging_restores_colors() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "a.exe");
        connected(&mut server, &mut host, 2, "quiet.exe");
        exchange(&mut server, &mut host, 1, SUBSCRIBE);
        exchange(&mut server, &mut host, 1, &set_colors(2, &[(30, RED)]));
        let lit = host.lighting.clone();
        let device = host.device.take();
        let unplugged = server.sync(&mut host);
        assert_eq!(event_payloads(&unplugged, 1), [device_event(DEVICE, false)]);
        assert_eq!(recipients(&unplugged), [1]);
        assert_eq!(get_devices(&mut server, &mut host, 1, 64), json!([]));
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &set_colors(3, &[(30, BLUE)]))), 6);
        assert!(server.sync(&mut host).is_empty(), "no repeat while still unplugged");
        host.device = device;
        let replugged = server.sync(&mut host);
        assert_eq!(event_payloads(&replugged, 1), [device_event(DEVICE, true)]);
        assert_eq!(host.lighting, lit);
    }

    #[test]
    fn blocking_a_connected_exe_closes_it() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "StreamDeck.exe");
        exchange(&mut server, &mut host, 1, &set_colors(2, &[(30, RED)]));
        host.policy.blocked = vec!["STREAMDECK.exe".into()];
        assert_eq!(server.sync(&mut host), [Output::Close { client: 1 }]);
        assert_eq!(host.lighting, ClientLighting::default());
        assert!(host.clients.is_empty());
        assert!(server.receive(1, HANDSHAKE.as_bytes(), &mut host).is_empty(), "a closed client is gone");
        server.disconnect(1, &mut host);
    }

    #[test]
    fn disabling_the_sdk_closes_connected_clients() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        host.policy.enabled = false;
        assert_eq!(server.sync(&mut host), [Output::Close { client: 1 }]);
    }

    #[test]
    fn refused_and_waiting_clients_stay_connected_on_sync() {
        let mut host = FakeHost {
            policy: Policy { blocked: vec!["blocked.exe".into()], ..Policy::default() },
            ..FakeHost::k100()
        };
        let mut server = Server::new(ServerConfig::default());
        server.connect(1, "blocked.exe".into(), &mut host);
        exchange(&mut server, &mut host, 1, HANDSHAKE);
        connected(&mut server, &mut host, 2, "game.exe");
        server.connect(3, "waiting.exe".into(), &mut host);
        assert!(server.sync(&mut host).is_empty());
        host.policy.enabled = false;
        assert_eq!(server.sync(&mut host), [Output::Close { client: 2 }]);
        assert_eq!(host.clients.iter().map(|summary| summary.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(exchange(&mut server, &mut host, 3, HANDSHAKE)[0]["result"]["state"], 4);
    }

    #[test]
    fn malformed_messages_get_an_error_reply_and_the_session_continues() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        assert_eq!(
            exchange(&mut server, &mut host, 1, r#"{"method":2,"code":8}"#),
            [json!({"code": 8, "error": 4, "method": 2, "result": {}})]
        );
        assert_eq!(get_devices(&mut server, &mut host, 1, 64).as_array().unwrap().len(), 1);
        assert_eq!(server.receive(1, b"{nope", &mut host), [Output::Close { client: 1 }]);
        assert!(host.clients.is_empty());
    }

    #[test]
    fn unknown_methods_are_incompatible_protocol() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        assert_eq!(
            exchange(&mut server, &mut host, 1, r#"{"method":42,"code":9}"#),
            [json!({"code": 9, "error": 3, "method": 42, "result": {}})]
        );
        assert_eq!(error(&exchange(&mut server, &mut host, 1, r#"{"method":17,"code":10}"#)), 3);
    }

    #[test]
    fn led_colors_read_back_the_clients_own_colors_and_white_where_it_set_none() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        connected(&mut server, &mut host, 2, "other.exe");
        let read =
            |luids: &str| format!(r#"{{"method":8,"code":2,"params":{{"deviceId":"{DEVICE}","luids":{luids}}}}}"#);
        assert_eq!(
            exchange(&mut server, &mut host, 1, &read("[30]"))[0]["result"],
            json!({"ledColors": [{"id": 30, "value": 4294967295u32}]})
        );
        exchange(&mut server, &mut host, 1, &set_colors(1, &[(30, RED)]));
        assert_eq!(
            exchange(&mut server, &mut host, 1, &read("[30, 42]"))[0]["result"],
            json!({"ledColors": [{"id": 30, "value": RED}, {"id": 42, "value": 4294967295u32}]})
        );
        assert_eq!(
            exchange(&mut server, &mut host, 2, &read("[30]"))[0]["result"],
            json!({"ledColors": [{"id": 30, "value": 4294967295u32}]})
        );
        assert_eq!(
            exchange(&mut server, &mut host, 1, &read("[2147418113, 30, 0]")),
            [json!({"code": 2, "error": 0, "method": 8, "result": {"ledColors": [{"id": 30, "value": RED}]}})]
        );
    }

    #[test]
    fn client_summaries_track_access_priority_and_intercepted_keys() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "StreamDeck.exe");
        assert_eq!(
            host.clients,
            [ClientSummary {
                id: 1,
                exe: "StreamDeck.exe".into(),
                refused: false,
                access: AccessLevel::Shared,
                priority: DEFAULT_PRIORITY,
                intercepted: vec![],
                subscribed: false,
            }]
        );
        exchange(&mut server, &mut host, 1, &control(1, 2));
        exchange(&mut server, &mut host, 1, &priority(2, 200));
        exchange(&mut server, &mut host, 1, &intercept(3, 3, true));
        exchange(&mut server, &mut host, 1, SUBSCRIBE);
        let expected = ClientSummary {
            id: 1,
            exe: "StreamDeck.exe".into(),
            refused: false,
            access: AccessLevel::ExclusiveKeys,
            priority: 200,
            intercepted: vec![MacroKey(3)],
            subscribed: true,
        };
        assert_eq!(host.clients, server.clients());
        assert_eq!(server.clients(), [expected]);
    }

    #[test]
    fn release_clears_the_layer_and_intercepts_and_keeps_the_priority() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        exchange(&mut server, &mut host, 1, &priority(1, 40));
        exchange(&mut server, &mut host, 1, &control(2, 2));
        exchange(&mut server, &mut host, 1, &intercept(3, 2, true));
        exchange(&mut server, &mut host, 1, &set_colors(4, &[(30, RED)]));
        assert_eq!(error(&exchange(&mut server, &mut host, 1, &release(5))), 0);
        assert_eq!((host.lighting.clone(), host.intercepted.clone()), (ClientLighting::default(), vec![]));
        assert_eq!((summary(&host, 1).access, summary(&host, 1).priority), (AccessLevel::Shared, 40));
    }

    #[test]
    fn a_disconnect_releases_layers_and_intercepts() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "StreamDeck.exe");
        exchange(&mut server, &mut host, 1, &control(1, 3));
        exchange(&mut server, &mut host, 1, &intercept(2, 1, true));
        exchange(&mut server, &mut host, 1, &set_colors(3, &[(1, 0xFF00_FF00)]));
        assert!(matches!(host.lighting, ClientLighting::Exclusive(_)));
        server.disconnect(1, &mut host);
        assert_eq!(
            (host.lighting.clone(), host.intercepted.clone(), host.clients.len()),
            (ClientLighting::default(), vec![], 0)
        );
        server.disconnect(1, &mut host);
        assert!(server.clients().is_empty());
    }

    #[test]
    fn the_host_hears_only_about_changes() {
        let (mut server, mut host) = server_and_host();
        connected(&mut server, &mut host, 1, "game.exe");
        exchange(&mut server, &mut host, 1, &set_colors(1, &[(30, RED)]));
        let calls = host.setter_calls;
        exchange(&mut server, &mut host, 1, &set_colors(2, &[(30, RED)]));
        exchange(&mut server, &mut host, 1, &get_devices_request());
        server.sync(&mut host);
        assert_eq!(host.setter_calls, calls);
    }

    fn get_devices_request() -> String {
        r#"{"method":1,"code":1,"params":{"deviceTypeMask":1,"sizeMax":64}}"#.into()
    }

    #[test]
    fn each_exe_is_noted_once_ignoring_case() {
        let mut server = Server::new(ServerConfig::default());
        assert!(server.note_first_connection("StreamDeck.exe"));
        assert!(!server.note_first_connection("streamdeck.EXE"));
        assert!(server.note_first_connection("game.exe"));
    }
}
