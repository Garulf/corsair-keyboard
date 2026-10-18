use crate::device::DeviceSnapshot;
use crate::host::{ClientSummary, Policy, SdkHost};
use crate::server::{ClientId, Output, Server};
use engine::ClientLighting;
use serde_json::Value;

pub(crate) const DEVICE: &str = "{00000000-0000-4000-8000-000000000001}";
pub(crate) const HANDSHAKE: &str = r#"{"method":0,"code":-1,"params":{"pid":1,"clientVersion":"4.0.84"}}"#;

pub(crate) struct FakeHost {
    pub device: Option<DeviceSnapshot>,
    pub policy: Policy,
    pub frame: Option<k100::Frame>,
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
            frame: None,
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

    fn current_frame(&self) -> Option<k100::Frame> {
        self.frame
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
