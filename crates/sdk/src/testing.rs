use crate::device::{DeviceSnapshot, macro_key_led};
use crate::host::{ClientSummary, Policy, SdkHost};
use crate::ids::{MacroKey, Method};
use crate::server::{ClientId, Output, Server, ServerConfig};
use crate::transcript::{Direction, Entry, Transcript};
use engine::ClientLighting;
use serde_json::Value;

pub(crate) const DEVICE: &str = "{00000000-0000-4000-8000-000000000001}";
const REPLAY_CLIENT: ClientId = 1;
const KEY_EVENT_ID: i64 = 2;
pub(crate) const HANDSHAKE: &str = r#"{"method":0,"code":-1,"params":{"pid":1,"clientVersion":"4.0.84"}}"#;

pub(crate) struct FakeHost {
    pub device: Option<DeviceSnapshot>,
    pub policy: Policy,
    pub lighting: ClientLighting,
    pub intercepted: Vec<u8>,
    pub clients: Vec<ClientSummary>,
    pub setter_calls: usize,
}

impl FakeHost {
    pub fn k100() -> Self {
        let device =
            DeviceSnapshot { id: DEVICE.into(), serial: "SERIAL".into(), physical_layout: 1, logical_layout: 2 };
        Self {
            device: Some(device),
            policy: Policy::default(),
            lighting: ClientLighting::default(),
            intercepted: Vec::new(),
            clients: Vec::new(),
            setter_calls: 0,
        }
    }
}

impl SdkHost for FakeHost {
    fn device(&self) -> Option<DeviceSnapshot> {
        self.device.clone()
    }

    fn policy(&self) -> Policy {
        self.policy.clone()
    }

    fn set_lighting(&mut self, lighting: ClientLighting) {
        self.setter_calls += 1;
        self.lighting = lighting;
    }

    fn set_intercepted(&mut self, leds: Vec<u8>) {
        self.setter_calls += 1;
        self.intercepted = leds;
    }

    fn clients_changed(&mut self, clients: Vec<ClientSummary>) {
        self.setter_calls += 1;
        self.clients = clients;
    }
}

pub(crate) fn exchange(server: &mut Server, host: &mut FakeHost, client: ClientId, json: &str) -> Vec<Value> {
    server
        .receive(client, json.as_bytes(), host)
        .into_iter()
        .filter_map(|output| match output {
            Output::Send { client: to, payload } if to == client => Some(serde_json::from_slice(&payload).unwrap()),
            _ => None,
        })
        .collect()
}

pub(crate) fn connected(server: &mut Server, host: &mut FakeHost, client: ClientId, exe: &str) {
    server.connect(client, exe.into(), host);
    let reply = exchange(server, host, client, HANDSHAKE);
    assert_eq!(reply[0]["result"]["state"], crate::ids::STATE_CONNECTED, "{exe} should connect");
}

pub(crate) fn set_colors(code: i64, colors: &[(u32, u32)]) -> String {
    let colors: Vec<Value> = colors.iter().map(|&(id, value)| serde_json::json!({"id": id, "value": value})).collect();
    serde_json::json!({"method": 10, "code": code, "params": {"deviceId": DEVICE, "ledColors": colors}}).to_string()
}

pub(crate) fn recipients(outputs: &[Output]) -> Vec<ClientId> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::Send { client, .. } => Some(*client),
            Output::Close { .. } => None,
        })
        .collect()
}

pub(crate) fn replay(capture: &Transcript, mut host: FakeHost) -> Transcript {
    let mut server = Server::new(ServerConfig::default());
    server.connect(REPLAY_CLIENT, "k100.exe".into(), &mut host);
    let mut entries = Vec::new();
    for entry in &capture.entries {
        let outputs = match entry.direction {
            Direction::Sent => {
                entries.push(entry.clone());
                server.receive(REPLAY_CLIENT, entry.message.to_string().as_bytes(), &mut host)
            }
            Direction::Received => match captured_key(&entry.message) {
                Some((led, pressed)) => server.key(led, pressed, &mut host),
                None => Vec::new(),
            },
            Direction::Note => Vec::new(),
        };
        entries.extend(outputs.into_iter().filter_map(|output| match output {
            Output::Send { client: REPLAY_CLIENT, payload } => Some(Entry {
                at_ms: entry.at_ms,
                direction: Direction::Received,
                message: serde_json::from_slice(&payload).unwrap(),
            }),
            _ => None,
        }));
    }
    Transcript { entries, ..capture.clone() }
}

fn captured_key(message: &Value) -> Option<(u8, bool)> {
    let result = &message["result"];
    if message["method"].as_i64() != Some(Method::Event.id()) || result["id"].as_i64() != Some(KEY_EVENT_ID) {
        return None;
    }
    let key = u8::try_from(result["keyEvent"]["keyId"].as_u64()?).ok()?;
    Some((macro_key_led(MacroKey(key))?, result["keyEvent"]["isPressed"].as_bool()?))
}
