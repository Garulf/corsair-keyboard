use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const EVENT_METHOD: i64 = 17;
const WILDCARD: &str = "*";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub tool: String,
    pub pipe: String,
    pub mode: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at_ms: u64,
    pub direction: Direction,
    pub message: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Sent,
    Received,
    Note,
}

impl Transcript {
    pub fn sent_methods(&self) -> impl Iterator<Item = i64> + '_ {
        self.messages(Direction::Sent).filter_map(|message| message["method"].as_i64())
    }

    pub fn unanswered(&self) -> Vec<(i64, i64)> {
        let answered: Vec<_> = self.messages(Direction::Received).filter_map(method_and_code).collect();
        self.messages(Direction::Sent).filter_map(method_and_code).filter(|key| !answered.contains(key)).collect()
    }

    fn messages(&self, direction: Direction) -> impl Iterator<Item = &Value> + '_ {
        self.entries.iter().filter(move |entry| entry.direction == direction).map(|entry| &entry.message)
    }

    fn replies(&self) -> Vec<((i64, i64), &Value)> {
        self.messages(Direction::Received)
            .filter_map(|message| method_and_code(message).map(|key| (key, message)))
            .filter(|((method, _), _)| *method != EVENT_METHOD)
            .collect()
    }

    fn event_count(&self) -> usize {
        self.messages(Direction::Received).filter(|message| message["method"].as_i64() == Some(EVENT_METHOD)).count()
    }
}

fn method_and_code(message: &Value) -> Option<(i64, i64)> {
    Some((message["method"].as_i64()?, message["code"].as_i64().unwrap_or(0)))
}

pub fn normalize(message: &Value) -> Value {
    match message {
        Value::Object(map) => {
            Value::Object(map.iter().map(|(key, value)| (key.clone(), normalize_member(key, value))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

fn normalize_member(key: &str, value: &Value) -> Value {
    match (key, value) {
        ("deviceId" | "serial" | "serverHostVersion" | "clientVersion", Value::String(_)) => json!(WILDCARD),
        ("pid", _) => json!(WILDCARD),
        ("deviceInfos", Value::Array(items)) => Value::Array(items.iter().map(wildcard_device_id).collect()),
        ("ledColors", Value::Array(items)) => Value::Array(items.iter().map(zero_led_value).collect()),
        _ => normalize(value),
    }
}

fn wildcard_device_id(info: &Value) -> Value {
    let mut info = normalize(info);
    if let Some(id) = info.get_mut("id").filter(|id| id.is_string()) {
        *id = json!(WILDCARD);
    }
    info
}

fn zero_led_value(color: &Value) -> Value {
    let mut color = normalize(color);
    if let Some(value) = color.get_mut("value") {
        *value = json!(0);
    }
    color
}

pub fn differences(expected: &Transcript, actual: &Transcript) -> Vec<String> {
    let actual_replies = actual.replies();
    let mut matched = vec![false; actual_replies.len()];
    let mut lines = Vec::new();
    for ((method, code), expected_message) in expected.replies() {
        let label = format!("method {method} code {code}");
        let found = actual_replies
            .iter()
            .enumerate()
            .find(|(position, (key, _))| !matched[*position] && *key == (method, code));
        let Some((position, (_, actual_message))) = found else {
            lines.push(format!("{label}: missing"));
            continue;
        };
        matched[position] = true;
        let mut value_lines = Vec::new();
        compare("", &normalize(expected_message), &normalize(actual_message), &mut value_lines);
        lines.extend(value_lines.into_iter().map(|line| format!("{label}: {line}")));
    }
    for (position, ((method, code), _)) in actual_replies.iter().enumerate() {
        if !matched[position] {
            lines.push(format!("method {method} code {code}: unexpected"));
        }
    }
    let (expected_events, actual_events) = (expected.event_count(), actual.event_count());
    if expected_events != actual_events {
        lines.push(format!("events: expected {expected_events}, got {actual_events}"));
    }
    lines
}

fn compare(path: &str, expected: &Value, actual: &Value, lines: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Object(left), Value::Object(right)) => {
            let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                match (left.get(key), right.get(key)) {
                    (Some(l), Some(r)) => compare(&child, l, r, lines),
                    (Some(_), None) => lines.push(format!("{child}: missing")),
                    _ => lines.push(format!("{child}: unexpected")),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            for position in 0..left.len().max(right.len()) {
                let child = format!("{path}[{position}]");
                match (left.get(position), right.get(position)) {
                    (Some(l), Some(r)) => compare(&child, l, r, lines),
                    (Some(_), None) => lines.push(format!("{child}: missing")),
                    _ => lines.push(format!("{child}: unexpected")),
                }
            }
        }
        (Value::Number(left), Value::Number(right)) if left.as_f64() == right.as_f64() => {}
        (left, right) if left == right => {}
        (left, right) => {
            let prefix = if path.is_empty() { String::new() } else { format!("{path}: ") };
            lines.push(format!("{prefix}expected {left}, got {right}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Transcript {
        serde_json::from_str(include_str!("../fixtures/handshake.json")).unwrap()
    }

    #[test]
    fn the_fixture_transcript_loads_and_matches_itself() {
        let transcript = fixture();
        assert_eq!(transcript.sent_methods().collect::<Vec<_>>(), [0, 1]);
        assert!(transcript.unanswered().is_empty());
        assert!(differences(&transcript, &transcript).is_empty());
    }

    #[test]
    fn a_sent_request_without_a_reply_is_unanswered() {
        let mut transcript = fixture();
        transcript.entries.truncate(3);
        assert_eq!(transcript.unanswered(), [(1, 1)]);
    }

    #[test]
    fn device_ids_serials_and_host_versions_are_normalized_away() {
        let a = json!({"result": {"deviceInfos": [{"id": "{a}", "serial": "1", "model": "K100 RGB"}], "serverHostVersion": "5.1"}});
        let b = json!({"result": {"deviceInfos": [{"id": "{b}", "serial": "2", "model": "K100 RGB"}], "serverHostVersion": "5.2"}});
        assert_eq!(normalize(&a), normalize(&b));
    }

    #[test]
    fn handshake_pid_and_client_version_are_normalized_away() {
        let a = json!({"params": {"pid": 1, "clientVersion": "4.0.1"}});
        let b = json!({"params": {"pid": 2, "clientVersion": "4.0.2"}});
        assert_eq!(normalize(&a), normalize(&b));
    }

    #[test]
    fn led_color_values_are_zeroed_but_their_ids_are_kept() {
        let a = json!({"result": {"ledColors": [{"id": 5, "value": 4278190080u32}]}});
        let b = json!({"result": {"ledColors": [{"id": 5, "value": 0}]}});
        let c = json!({"result": {"ledColors": [{"id": 6, "value": 0}]}});
        assert_eq!(normalize(&a), normalize(&b));
        assert_ne!(normalize(&a), normalize(&c));
    }

    #[test]
    fn the_event_id_is_not_mistaken_for_a_device_id() {
        let a = json!({"result": {"id": 1}});
        let b = json!({"result": {"id": 2}});
        assert_ne!(normalize(&a), normalize(&b));
    }

    #[test]
    fn differing_positions_are_reported_with_their_path() {
        let base = fixture();
        let mut other = base.clone();
        other.entries[3].message["result"]["deviceInfos"][0]["model"] = json!("K100");
        assert_eq!(
            differences(&base, &other),
            ["method 1 code 1: result.deviceInfos[0].model: expected \"K70 MAX\", got \"K100\""]
        );
        other.entries.truncate(3);
        assert_eq!(differences(&base, &other), ["method 1 code 1: missing"]);
    }

    #[test]
    fn numbers_are_compared_by_value_and_not_by_spelling() {
        let mut base = fixture();
        base.entries[3].message["result"]["x"] = json!(30);
        let mut other = base.clone();
        other.entries[3].message["result"]["x"] = json!(30.0);
        assert!(differences(&base, &other).is_empty());
        other.entries[3].message["result"]["x"] = json!(30.5);
        assert_eq!(differences(&base, &other), ["method 1 code 1: result.x: expected 30, got 30.5"]);
    }

    #[test]
    fn extra_replies_and_missing_keys_are_reported() {
        let base = fixture();
        let mut other = base.clone();
        other.entries[3].message["result"]["extra"] = json!(1);
        other.entries.push(Entry {
            at_ms: 30,
            direction: Direction::Received,
            message: json!({"method": 9, "code": 4, "error": 0}),
        });
        assert_eq!(
            differences(&base, &other),
            ["method 1 code 1: result.extra: unexpected", "method 9 code 4: unexpected"]
        );
        assert_eq!(differences(&other, &base), ["method 1 code 1: result.extra: missing", "method 9 code 4: missing"]);
    }

    #[test]
    fn events_are_counted_and_not_paired() {
        let base = fixture();
        let mut other = base.clone();
        other.entries.push(Entry {
            at_ms: 40,
            direction: Direction::Received,
            message: json!({"method": 17, "result": {"id": 2}}),
        });
        assert_eq!(differences(&base, &other), ["events: expected 0, got 1"]);
    }

    #[test]
    fn notes_and_directions_serialize_in_lowercase() {
        let entry = Entry { at_ms: 1, direction: Direction::Note, message: json!("skipped") };
        assert_eq!(serde_json::to_string(&entry).unwrap(), r#"{"at_ms":1,"direction":"note","message":"skipped"}"#);
    }
}
