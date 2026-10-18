use crate::ids::{Method, STATE_CONNECTED};
use crate::transcript::{Direction, Entry, Transcript};
use serde_json::{Value, json};
use std::io;
use std::time::{Duration, Instant};

pub const READ_ONLY_METHODS: [Method; 10] = [
    Method::Handshake,
    Method::GetDevices,
    Method::GetDeviceInfo,
    Method::GetPropertyInfo,
    Method::ReadProperty,
    Method::LuidForKeyName,
    Method::GetLedPositions,
    Method::GetLedColors,
    Method::Subscribe,
    Method::Unsubscribe,
];

const CLIENT_VERSION: &str = "4.0.84";
const HANDSHAKE_CODE: i64 = -1;
const KEYBOARD_TYPE: i64 = 1;
const DEVICE_TYPE_MASKS: [i64; 3] = [4_294_967_295, 1, -1];
const DEVICE_LIMIT: i64 = 64;
const LED_LIMIT: i64 = 512;
const PROPERTY_IDS: std::ops::RangeInclusive<i64> = 1..=13;
const UNKNOWN_PROPERTY: i64 = 99;
const NO_DEVICE: &str = "{00000000-0000-0000-0000-000000000000}";
const KEY_NAMES: [&str; 9] = ["A", "a", "W", "Z", "1", "0", ";", "\\", " "];
const KEY_CODE: i64 = 65;
const WRITE_PRIORITY: i64 = 200;
const WRITE_PAUSE: Duration = Duration::from_secs(1);
const RED: u32 = 0xFFFF_0000;
const BLUE: u32 = 0xFF00_00FF;
const LIGHTING_LEVEL: i64 = 1;
const KEYS_LEVEL: i64 = 2;
const INTERCEPTED_KEY: i64 = 1;

pub trait ProbePipe {
    fn send(&mut self, payload: &[u8]) -> io::Result<()>;
    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOptions {
    pub write: bool,
    pub listen: Duration,
    pub pid: u32,
    pub handshake_timeout: Duration,
    pub reply_timeout: Duration,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            write: false,
            listen: Duration::from_secs(15),
            pid: 0,
            handshake_timeout: Duration::from_secs(60),
            reply_timeout: Duration::from_secs(5),
        }
    }
}

pub fn run_probe(pipe: &mut dyn ProbePipe, options: &ProbeOptions, on_listen: &mut dyn FnMut()) -> Transcript {
    let mut probe = Probe { pipe, options, started: Instant::now(), entries: Vec::new(), next_code: 1 };
    if let Err(error) = probe.sequence(on_listen) {
        probe.note(json!({"error": error.to_string()}));
    }
    Transcript {
        tool: format!("k100 diag sdk-probe {}", env!("CARGO_PKG_VERSION")),
        pipe: String::new(),
        mode: if options.write { "write" } else { "read-only" }.into(),
        entries: probe.entries,
    }
}

struct Probe<'a> {
    pipe: &'a mut dyn ProbePipe,
    options: &'a ProbeOptions,
    started: Instant,
    entries: Vec<Entry>,
    next_code: i64,
}

impl Probe<'_> {
    fn sequence(&mut self, on_listen: &mut dyn FnMut()) -> io::Result<()> {
        if !self.handshake()? {
            return Ok(());
        }
        let device_ids = self.list_devices()?;
        for id in &device_ids {
            self.call(Method::GetDeviceInfo, json!({"deviceId": id}))?;
        }
        self.call(Method::GetDeviceInfo, json!({"deviceId": NO_DEVICE}))?;
        if let Some(device) = device_ids.first() {
            let led_ids = self.inspect(device)?;
            if self.options.write {
                self.write_sequence(device, &led_ids)?;
            }
        }
        self.call(Method::Subscribe, json!({}))?;
        on_listen();
        self.collect(self.options.listen)?;
        self.call(Method::Unsubscribe, json!({}))?;
        if self.options.write
            && let Some(device) = device_ids.first()
        {
            self.call(Method::ReleaseControl, json!({"deviceId": device}))?;
        }
        Ok(())
    }

    fn handshake(&mut self) -> io::Result<bool> {
        let params = json!({"pid": self.options.pid, "clientVersion": CLIENT_VERSION});
        let timeout = self.options.handshake_timeout;
        let reply = self.exchange(Method::Handshake, HANDSHAKE_CODE, params, timeout)?;
        Ok(reply.is_some_and(|reply| reply["result"]["state"] == STATE_CONNECTED))
    }

    fn list_devices(&mut self) -> io::Result<Vec<String>> {
        let mut ids: Vec<String> = Vec::new();
        for mask in DEVICE_TYPE_MASKS {
            let params = json!({"deviceTypeMask": mask, "sizeMax": DEVICE_LIMIT});
            let reply = self.call(Method::GetDevices, params)?;
            for id in reply.iter().flat_map(keyboard_ids) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }

    fn inspect(&mut self, device: &str) -> io::Result<Vec<Value>> {
        for property in PROPERTY_IDS {
            let params = json!({"deviceId": device, "propertyId": property, "index": 0});
            let info = self.call(Method::GetPropertyInfo, params.clone())?;
            if info.is_some_and(|info| info["error"] == 0) {
                self.call(Method::ReadProperty, params)?;
            }
        }
        self.call(Method::ReadProperty, json!({"deviceId": device, "propertyId": UNKNOWN_PROPERTY, "index": 0}))?;
        for name in KEY_NAMES.iter().map(|name| json!(name)).chain([json!(KEY_CODE)]) {
            self.call(Method::LuidForKeyName, json!({"deviceId": device, "keyName": name}))?;
        }
        let positions = self.call(Method::GetLedPositions, json!({"deviceId": device, "sizeMax": LED_LIMIT}))?;
        let led_ids: Vec<Value> = positions
            .iter()
            .filter_map(|reply| reply["result"]["ledPositions"].as_array())
            .flatten()
            .map(|position| position["id"].clone())
            .collect();
        self.call(Method::GetLedColors, json!({"deviceId": device, "luids": led_ids}))?;
        self.call(Method::GetLedColors, json!({"deviceId": device, "luids": [0]}))?;
        Ok(led_ids)
    }

    fn write_sequence(&mut self, device: &str, led_ids: &[Value]) -> io::Result<()> {
        self.call(Method::SetLayerPriority, json!({"priority": WRITE_PRIORITY}))?;
        self.paint(device, led_ids, RED)?;
        self.collect(WRITE_PAUSE)?;
        self.request_control(device, LIGHTING_LEVEL)?;
        self.paint(device, led_ids, BLUE)?;
        self.collect(WRITE_PAUSE)?;
        self.call(Method::ReleaseControl, json!({"deviceId": device}))?;
        self.request_control(device, KEYS_LEVEL)?;
        let intercept = json!({"deviceId": device, "keyId": INTERCEPTED_KEY, "isIntercepted": true});
        self.call(Method::ConfigureKeyEvent, intercept)?;
        Ok(())
    }

    fn paint(&mut self, device: &str, led_ids: &[Value], value: u32) -> io::Result<()> {
        let colors: Vec<Value> = led_ids.iter().map(|id| json!({"id": id, "value": value})).collect();
        self.call(Method::SetLedColors, json!({"deviceId": device, "ledColors": colors})).map(drop)
    }

    fn request_control(&mut self, device: &str, level: i64) -> io::Result<()> {
        self.call(Method::RequestControl, json!({"deviceId": device, "accessLevel": level})).map(drop)
    }

    fn call(&mut self, method: Method, params: Value) -> io::Result<Option<Value>> {
        let code = self.next_code;
        self.next_code += 1;
        let timeout = self.options.reply_timeout;
        self.exchange(method, code, params, timeout)
    }

    fn exchange(&mut self, method: Method, code: i64, params: Value, timeout: Duration) -> io::Result<Option<Value>> {
        let request = json!({"method": method.id(), "code": code, "params": params});
        self.pipe.send(&serde_json::to_vec(&request).expect("a JSON value always serializes"))?;
        self.record(Direction::Sent, request);
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.pipe.receive(remaining)? {
                Some(payload) => {
                    let message = self.record_received(&payload);
                    if message["method"] == method.id() && message["code"] == code {
                        return Ok(Some(message));
                    }
                }
                None if remaining.is_zero() => {
                    self.note(json!({"timeout": {"method": method.id(), "code": code}}));
                    return Ok(None);
                }
                None => {}
            }
        }
    }

    fn collect(&mut self, duration: Duration) -> io::Result<()> {
        let deadline = Instant::now() + duration;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(payload) = self.pipe.receive(remaining)? {
                self.record_received(&payload);
            } else if remaining.is_zero() {
                return Ok(());
            }
        }
    }

    fn record_received(&mut self, payload: &[u8]) -> Value {
        let message = serde_json::from_slice(payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(payload).into_owned()));
        self.record(Direction::Received, message.clone());
        message
    }

    fn note(&mut self, message: Value) {
        self.record(Direction::Note, message);
    }

    fn record(&mut self, direction: Direction, message: Value) {
        let at_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.entries.push(Entry { at_ms, direction, message });
    }
}

fn keyboard_ids(reply: &Value) -> Vec<String> {
    reply["result"]["deviceInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|info| info["type"] == KEYBOARD_TYPE)
        .filter_map(|info| info["id"].as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Policy;
    use crate::server::{ClientId, Output, Server, ServerConfig};
    use crate::testing::FakeHost;
    use std::collections::{BTreeSet, VecDeque};

    const CLIENT: ClientId = 1;
    const FIRST_MACRO_LED: u8 = 131;

    struct ServerPipe {
        server: Server,
        host: FakeHost,
        inbox: VecDeque<Vec<u8>>,
        silent_method: Option<i64>,
    }

    impl ServerPipe {
        fn new(mut host: FakeHost) -> Self {
            let mut server = Server::new(ServerConfig::default());
            server.connect(CLIENT, "k100.exe".into(), &mut host);
            Self { server, host, inbox: VecDeque::new(), silent_method: None }
        }

        fn dropping_replies_to(mut self, method: i64) -> Self {
            self.silent_method = Some(method);
            self
        }

        fn queue(&mut self, outputs: Vec<Output>) {
            for output in outputs {
                if let Output::Send { client: CLIENT, payload } = output {
                    self.inbox.push_back(payload);
                }
            }
        }
    }

    impl ProbePipe for ServerPipe {
        fn send(&mut self, payload: &[u8]) -> io::Result<()> {
            let method = serde_json::from_slice::<Value>(payload).unwrap()["method"].as_i64().unwrap();
            let outputs = self.server.receive(CLIENT, payload, &mut self.host);
            let silent = self.silent_method == Some(method);
            let replies = outputs.into_iter().filter(|_| !silent).collect();
            self.queue(replies);
            if method == Method::Subscribe.id() {
                let pressed = self.server.key(FIRST_MACRO_LED, true, &mut self.host);
                let released = self.server.key(FIRST_MACRO_LED, false, &mut self.host);
                self.queue(pressed);
                self.queue(released);
            }
            Ok(())
        }

        fn receive(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
            if self.inbox.is_empty() {
                std::thread::sleep(timeout.min(Duration::from_millis(2)));
            }
            Ok(self.inbox.pop_front())
        }
    }

    struct BrokenPipe;

    impl ProbePipe for BrokenPipe {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"))
        }

        fn receive(&mut self, _: Duration) -> io::Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }

    fn quiet() -> ProbeOptions {
        ProbeOptions { listen: Duration::ZERO, ..ProbeOptions::default() }
    }

    fn sent(transcript: &Transcript, method: Method) -> Vec<&Value> {
        transcript
            .entries
            .iter()
            .filter(|entry| entry.direction == Direction::Sent && entry.message["method"] == method.id())
            .map(|entry| &entry.message)
            .collect()
    }

    #[test]
    fn the_default_probe_sends_only_read_only_methods() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let sent: BTreeSet<i64> = transcript.sent_methods().collect();
        assert_eq!(sent, READ_ONLY_METHODS.iter().map(|method| method.id()).collect());
        assert_eq!(transcript.entries[0].message["method"], 0);
        assert!(transcript.unanswered().is_empty(), "{:?}", transcript.unanswered());
        assert_eq!(transcript.mode, "read-only");
        assert_eq!(transcript.tool, format!("k100 diag sdk-probe {}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn the_handshake_carries_the_pid_and_the_public_client_version() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let options = ProbeOptions { pid: 4242, ..quiet() };
        let transcript = run_probe(&mut pipe, &options, &mut || {});
        assert_eq!(
            transcript.entries[0].message,
            json!({"method": 0, "code": -1, "params": {"pid": 4242, "clientVersion": "4.0.84"}})
        );
    }

    #[test]
    fn codes_count_up_from_one_after_the_handshake() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let codes: Vec<i64> = transcript
            .entries
            .iter()
            .filter(|entry| entry.direction == Direction::Sent)
            .map(|entry| entry.message["code"].as_i64().unwrap())
            .collect();
        assert_eq!(codes[0], -1);
        assert_eq!(codes[1..], (1..codes.len() as i64).collect::<Vec<_>>());
    }

    #[test]
    fn the_device_list_is_asked_for_with_three_masks() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let masks: Vec<&Value> =
            sent(&transcript, Method::GetDevices).into_iter().map(|m| &m["params"]["deviceTypeMask"]).collect();
        assert_eq!(masks, [&json!(4294967295u32), &json!(1), &json!(-1)]);
    }

    #[test]
    fn every_property_is_probed_and_only_readable_ones_are_read() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let infos = sent(&transcript, Method::GetPropertyInfo);
        let properties: Vec<i64> = infos.iter().map(|m| m["params"]["propertyId"].as_i64().unwrap()).collect();
        assert_eq!(properties, (1..=13).collect::<Vec<_>>());
        let read: Vec<i64> = sent(&transcript, Method::ReadProperty)
            .iter()
            .map(|m| m["params"]["propertyId"].as_i64().unwrap())
            .collect();
        assert_eq!(read.last(), Some(&99));
        assert!(read.len() > 1, "properties with error 0 are read: {read:?}");
        assert!(read.len() <= 14);
    }

    #[test]
    fn key_names_are_asked_for_as_text_and_as_a_code() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let names: Vec<&Value> =
            sent(&transcript, Method::LuidForKeyName).into_iter().map(|m| &m["params"]["keyName"]).collect();
        let expected: Vec<Value> =
            ["A", "a", "W", "Z", "1", "0", ";", "\\", " "].iter().map(|name| json!(name)).chain([json!(65)]).collect();
        assert_eq!(names, expected.iter().collect::<Vec<_>>());
    }

    #[test]
    fn led_colors_are_read_for_every_listed_led_and_then_for_a_bogus_one() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let positions = transcript
            .entries
            .iter()
            .find(|entry| entry.direction == Direction::Received && entry.message["method"] == 7)
            .unwrap();
        let listed = positions.message["result"]["ledPositions"].as_array().unwrap().len();
        let requests = sent(&transcript, Method::GetLedColors);
        assert_eq!(requests[0]["params"]["luids"].as_array().unwrap().len(), listed);
        assert!(listed > 100);
        assert_eq!(requests[1]["params"]["luids"], json!([0]));
    }

    #[test]
    fn unknown_ids_are_probed_on_a_second_device_id_of_zeros() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let ids: Vec<&Value> =
            sent(&transcript, Method::GetDeviceInfo).into_iter().map(|m| &m["params"]["deviceId"]).collect();
        assert_eq!(ids, [&json!(crate::testing::DEVICE), &json!("{00000000-0000-0000-0000-000000000000}")]);
    }

    #[test]
    fn key_events_during_the_listen_window_are_recorded() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let mut listened = false;
        let options = ProbeOptions { listen: Duration::from_millis(50), ..ProbeOptions::default() };
        let transcript = run_probe(&mut pipe, &options, &mut || listened = true);
        let keys: Vec<&Value> =
            transcript.entries.iter().filter(|e| e.message["method"] == 17).map(|e| &e.message).collect();
        assert!(listened);
        assert_eq!(
            keys.iter().map(|m| m["result"]["keyEvent"]["isPressed"].clone()).collect::<Vec<_>>(),
            [json!(true), json!(false)]
        );
    }

    #[test]
    fn a_refused_handshake_ends_the_probe() {
        let mut pipe =
            ServerPipe::new(FakeHost { policy: Policy { enabled: false, ..Policy::default() }, ..FakeHost::k100() });
        let transcript = run_probe(&mut pipe, &ProbeOptions::default(), &mut || {});
        assert_eq!(transcript.sent_methods().collect::<Vec<_>>(), [0]);
    }

    #[test]
    fn a_probe_without_a_keyboard_still_subscribes() {
        let mut pipe = ServerPipe::new(FakeHost { device: None, ..FakeHost::k100() });
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let methods: Vec<i64> = transcript.sent_methods().collect();
        assert_eq!(methods, [0, 1, 1, 1, 2, 14, 15]);
    }

    #[test]
    fn write_mode_adds_lighting_control_and_interception() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let options = ProbeOptions { write: true, ..quiet() };
        let transcript = run_probe(&mut pipe, &options, &mut || {});
        let sent_methods: BTreeSet<i64> = transcript.sent_methods().collect();
        assert!([9, 10, 12, 13, 16].iter().all(|method| sent_methods.contains(method)), "{sent_methods:?}");
        assert_eq!(transcript.mode, "write");
        assert!(transcript.unanswered().is_empty(), "{:?}", transcript.unanswered());
        let levels: Vec<&Value> =
            sent(&transcript, Method::RequestControl).into_iter().map(|m| &m["params"]["accessLevel"]).collect();
        assert_eq!(levels, [&json!(1), &json!(2)]);
        assert_eq!(sent(&transcript, Method::SetLayerPriority)[0]["params"]["priority"], 200);
        let colors: Vec<&Value> = sent(&transcript, Method::SetLedColors)
            .into_iter()
            .map(|m| &m["params"]["ledColors"][0]["value"])
            .collect();
        assert_eq!(colors, [&json!(0xFFFF0000u32), &json!(0xFF0000FFu32)]);
        assert_eq!(
            sent(&transcript, Method::ConfigureKeyEvent)[0]["params"],
            json!({
                "deviceId": crate::testing::DEVICE, "keyId": 1, "isIntercepted": true
            })
        );
        assert_eq!(sent(&transcript, Method::ReleaseControl).len(), 2);
        assert_eq!(transcript.sent_methods().last(), Some(Method::ReleaseControl.id()));
    }

    #[test]
    fn write_mode_replies_are_all_successes_against_the_server_core() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let options = ProbeOptions { write: true, ..quiet() };
        let transcript = run_probe(&mut pipe, &options, &mut || {});
        let failed: Vec<i64> = transcript
            .entries
            .iter()
            .filter(|e| e.direction == Direction::Received)
            .filter(|e| [9, 10, 12, 13, 16].iter().any(|method| e.message["method"] == *method))
            .filter(|e| e.message["error"] != 0)
            .map(|e| e.message["method"].as_i64().unwrap())
            .collect();
        assert!(failed.is_empty(), "{failed:?}");
    }

    #[test]
    fn a_server_that_never_answers_is_noted_and_the_probe_goes_on() {
        let mut pipe = ServerPipe::new(FakeHost::k100()).dropping_replies_to(7);
        let options = ProbeOptions { reply_timeout: Duration::from_millis(10), ..quiet() };
        let transcript = run_probe(&mut pipe, &options, &mut || {});
        assert!(
            transcript.entries.iter().any(|e| e.direction == Direction::Note && e.message["timeout"]["method"] == 7)
        );
        assert!(transcript.sent_methods().any(|method| method == 14), "the probe went on after the timeout");
    }

    #[test]
    fn an_io_error_is_noted_and_ends_the_run() {
        let transcript = run_probe(&mut BrokenPipe, &quiet(), &mut || {});
        let last = transcript.entries.last().unwrap();
        assert_eq!(last.direction, Direction::Note);
        assert_eq!(last.message, json!({"error": "pipe closed"}));
        assert!(transcript.sent_methods().next().is_none());
    }

    #[test]
    fn transcripts_survive_a_json_round_trip() {
        let mut pipe = ServerPipe::new(FakeHost::k100());
        let transcript = run_probe(&mut pipe, &quiet(), &mut || {});
        let text = serde_json::to_string_pretty(&transcript).unwrap();
        assert_eq!(serde_json::from_str::<Transcript>(&text).unwrap(), transcript);
    }
}
