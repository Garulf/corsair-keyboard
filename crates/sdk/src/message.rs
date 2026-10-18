use crate::ids::{ErrorCode, Luid, MacroKey, Method};
use k100::Rgb;
use serde_json::{Map, Value, json};

const EVENT_METHOD: i64 = 17;
const KEY_EVENT_ID: i64 = 2;
const DEVICE_EVENT_ID: i64 = 1;
const EVENT_CODE: i64 = -1;
const EXACT_FLOAT_LIMIT: f64 = 9_007_199_254_740_992.0;
const NO_DEVICE_TYPES: u32 = 0;

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub method: i64,
    pub code: i64,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("payload is not JSON: {0}")]
    NotJson(String),
    #[error("payload has no integer method")]
    NoMethod,
}

pub fn parse_envelope(payload: &[u8]) -> Result<Envelope, EnvelopeError> {
    let value: Value = serde_json::from_slice(payload).map_err(|e| EnvelopeError::NotJson(e.to_string()))?;
    let Value::Object(mut object) = value else {
        return Err(EnvelopeError::NoMethod);
    };
    let method = object.get("method").and_then(Value::as_i64).ok_or(EnvelopeError::NoMethod)?;
    let code = object.get("code").and_then(Value::as_i64).unwrap_or(0);
    let params = match object.remove("params") {
        Some(Value::Object(params)) => params,
        _ => Map::new(),
    };
    Ok(Envelope { method, code, params })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedColor {
    pub luid: Luid,
    pub rgb: Rgb,
    pub alpha: u8,
}

impl LedColor {
    pub fn from_value(luid: Luid, value: u32) -> Self {
        Self { luid, rgb: Rgb::new((value >> 16) as u8, (value >> 8) as u8, value as u8), alpha: (value >> 24) as u8 }
    }

    pub fn value(&self) -> u32 {
        (self.alpha as u32) << 24 | (self.rgb.r as u32) << 16 | (self.rgb.g as u32) << 8 | self.rgb.b as u32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Handshake { pid: Option<i64>, client_version: Option<String> },
    GetDevices { type_mask: u32, size_max: i64 },
    GetDeviceInfo { device_id: String },
    GetPropertyInfo { device_id: String, property: i64, index: u32 },
    ReadProperty { device_id: String, property: i64, index: u32 },
    WriteProperty { device_id: String, property: i64, index: u32 },
    LuidForKeyName { device_id: String, key: char },
    GetLedPositions { device_id: String, size_max: i64 },
    GetLedColors { device_id: String, luids: Vec<Luid> },
    SetLayerPriority { priority: i64 },
    SetLedColors { device_id: String, colors: Vec<LedColor> },
    FlushLedColors { devices: Vec<(String, Vec<LedColor>)> },
    RequestControl { device_id: String, level: i64 },
    ReleaseControl { device_id: String },
    Subscribe,
    Unsubscribe,
    ConfigureKeyEvent { device_id: String, key: i64, intercepted: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    #[error("unknown method {0}")]
    UnknownMethod(i64),
    #[error("bad params: {0}")]
    BadParams(String),
}

pub fn parse_request(envelope: &Envelope) -> Result<Request, RequestError> {
    let fields = Fields::root(&envelope.params);
    match Method::from_id(envelope.method) {
        None | Some(Method::Event) => Err(RequestError::UnknownMethod(envelope.method)),
        Some(Method::Handshake) => Ok(Request::Handshake {
            pid: fields.optional_integer("pid")?,
            client_version: fields.optional_string("clientVersion")?,
        }),
        Some(Method::GetDevices) => {
            Ok(Request::GetDevices { type_mask: fields.device_type_mask()?, size_max: fields.integer("sizeMax")? })
        }
        Some(Method::GetDeviceInfo) => Ok(Request::GetDeviceInfo { device_id: fields.string("deviceId")? }),
        Some(Method::GetPropertyInfo) => Ok(Request::GetPropertyInfo {
            device_id: fields.string("deviceId")?,
            property: fields.integer("propertyId")?,
            index: fields.index()?,
        }),
        Some(Method::ReadProperty) => Ok(Request::ReadProperty {
            device_id: fields.string("deviceId")?,
            property: fields.integer("propertyId")?,
            index: fields.index()?,
        }),
        Some(Method::WriteProperty) => Ok(Request::WriteProperty {
            device_id: fields.string("deviceId")?,
            property: fields.integer("propertyId")?,
            index: fields.index()?,
        }),
        Some(Method::LuidForKeyName) => {
            Ok(Request::LuidForKeyName { device_id: fields.string("deviceId")?, key: fields.key_name()? })
        }
        Some(Method::GetLedPositions) => {
            Ok(Request::GetLedPositions { device_id: fields.string("deviceId")?, size_max: fields.integer("sizeMax")? })
        }
        Some(Method::GetLedColors) => {
            Ok(Request::GetLedColors { device_id: fields.string("deviceId")?, luids: fields.luids("luids")? })
        }
        Some(Method::SetLayerPriority) => Ok(Request::SetLayerPriority { priority: fields.integer("priority")? }),
        Some(Method::SetLedColors) => {
            Ok(Request::SetLedColors { device_id: fields.string("deviceId")?, colors: fields.led_colors()? })
        }
        Some(Method::FlushLedColors) => {
            let devices = fields
                .objects("devices")?
                .iter()
                .map(|device| Ok((device.string("deviceId")?, device.led_colors()?)))
                .collect::<Result<_, RequestError>>()?;
            Ok(Request::FlushLedColors { devices })
        }
        Some(Method::RequestControl) => {
            Ok(Request::RequestControl { device_id: fields.string("deviceId")?, level: fields.integer("accessLevel")? })
        }
        Some(Method::ReleaseControl) => Ok(Request::ReleaseControl { device_id: fields.string("deviceId")? }),
        Some(Method::Subscribe) => Ok(Request::Subscribe),
        Some(Method::Unsubscribe) => Ok(Request::Unsubscribe),
        Some(Method::ConfigureKeyEvent) => Ok(Request::ConfigureKeyEvent {
            device_id: fields.string("deviceId")?,
            key: fields.integer("keyId")?,
            intercepted: fields.boolean("isIntercepted")?,
        }),
    }
}

struct Fields<'a> {
    map: &'a Map<String, Value>,
    prefix: String,
}

impl<'a> Fields<'a> {
    fn root(map: &'a Map<String, Value>) -> Self {
        Self { map, prefix: String::new() }
    }

    fn bad(&self, key: &str, expected: &str) -> RequestError {
        RequestError::BadParams(format!("{}{key} is missing or not {expected}", self.prefix))
    }

    fn string(&self, key: &str) -> Result<String, RequestError> {
        self.optional_string(key)?.ok_or_else(|| self.bad(key, "a string"))
    }

    fn optional_string(&self, key: &str) -> Result<Option<String>, RequestError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(self.bad(key, "a string")),
        }
    }

    fn integer(&self, key: &str) -> Result<i64, RequestError> {
        self.optional_integer(key)?.ok_or_else(|| self.bad(key, "an integer"))
    }

    fn optional_integer(&self, key: &str) -> Result<Option<i64>, RequestError> {
        match self.map.get(key) {
            None => Ok(None),
            Some(value) => integral(value).map(Some).ok_or_else(|| self.bad(key, "an integer")),
        }
    }

    fn unsigned(&self, key: &str) -> Result<u32, RequestError> {
        self.map.get(key).and_then(unsigned32).ok_or_else(|| self.bad(key, "a 32-bit integer"))
    }

    fn device_type_mask(&self) -> Result<u32, RequestError> {
        let mask = self.integer("deviceTypeMask")?;
        Ok(i32::try_from(mask).map_or(NO_DEVICE_TYPES, |mask| mask as u32))
    }

    fn index(&self) -> Result<u32, RequestError> {
        match self.map.get("index") {
            None => Ok(0),
            Some(_) => self.unsigned("index"),
        }
    }

    fn boolean(&self, key: &str) -> Result<bool, RequestError> {
        self.map.get(key).and_then(Value::as_bool).ok_or_else(|| self.bad(key, "a boolean"))
    }

    fn key_name(&self) -> Result<char, RequestError> {
        self.map
            .get("keyName")
            .and_then(integral)
            .filter(|code| (0..=127).contains(code))
            .map(|code| code as u8 as char)
            .ok_or_else(|| self.bad("keyName", "a character code from 0 to 127"))
    }

    fn array(&self, key: &str) -> Result<&'a [Value], RequestError> {
        match self.map.get(key) {
            Some(Value::Array(items)) => Ok(items),
            _ => Err(self.bad(key, "an array")),
        }
    }

    fn objects(&self, key: &str) -> Result<Vec<Fields<'a>>, RequestError> {
        self.array(key)?
            .iter()
            .enumerate()
            .map(|(position, item)| match item {
                Value::Object(map) => Ok(Fields { map, prefix: format!("{}{key}[{position}].", self.prefix) }),
                _ => Err(self.bad(&format!("{key}[{position}]"), "an object")),
            })
            .collect()
    }

    fn luids(&self, key: &str) -> Result<Vec<Luid>, RequestError> {
        self.array(key)?
            .iter()
            .enumerate()
            .map(|(position, item)| {
                unsigned32(item).map(Luid).ok_or_else(|| self.bad(&format!("{key}[{position}]"), "a 32-bit integer"))
            })
            .collect()
    }

    fn led_colors(&self) -> Result<Vec<LedColor>, RequestError> {
        self.objects("ledColors")?
            .iter()
            .map(|item| Ok(LedColor::from_value(Luid(item.unsigned("id")?), item.unsigned("value")?)))
            .collect()
    }
}

fn integral(value: &Value) -> Option<i64> {
    if let Some(integer) = value.as_i64() {
        return Some(integer);
    }
    let float = value.as_f64()?;
    (float.fract() == 0.0 && float.abs() < EXACT_FLOAT_LIMIT).then_some(float as i64)
}

fn unsigned32(value: &Value) -> Option<u32> {
    let integer = integral(value)?;
    (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&integer).then_some(integer as u32)
}

pub fn reply(method: i64, code: i64, error: ErrorCode, result: Value) -> Vec<u8> {
    to_bytes(&json!({"code": code, "error": error.code(), "method": method, "result": result}))
}

pub fn key_event(device_id: &str, key: MacroKey, pressed: bool) -> Vec<u8> {
    let event = json!({"deviceId": device_id, "isPressed": pressed, "keyId": key.0});
    event_payload(json!({"id": KEY_EVENT_ID, "keyEvent": event}))
}

pub fn device_event(device_id: &str, connected: bool) -> Vec<u8> {
    let event = json!({"deviceId": device_id, "isConnected": connected});
    event_payload(json!({"id": DEVICE_EVENT_ID, "deviceConnectionStatusChangedEvent": event}))
}

fn event_payload(result: Value) -> Vec<u8> {
    reply(EVENT_METHOD, EVENT_CODE, ErrorCode::Success, result)
}

pub fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < EXACT_FLOAT_LIMIT {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn to_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("a JSON value always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::device_type;

    fn request(text: &str) -> Result<Request, RequestError> {
        parse_request(&parse_envelope(text.as_bytes()).unwrap())
    }

    #[test]
    fn the_handshake_parses_with_code_minus_one() {
        let envelope =
            parse_envelope(br#"{"method":0,"code":-1,"params":{"pid":4242,"clientVersion":"4.0.84"}}"#).unwrap();
        assert_eq!((envelope.method, envelope.code), (0, -1));
        assert_eq!(
            parse_request(&envelope).unwrap(),
            Request::Handshake { pid: Some(4242), client_version: Some("4.0.84".into()) }
        );
    }

    #[test]
    fn the_handshake_reply_matches_the_soup_sample_byte_for_byte() {
        let result = json!({"serverHostVersion": "5.7.106", "serverVersion": "4.0.18", "state": 6});
        assert_eq!(
            reply(0, -1, ErrorCode::Success, result),
            br#"{"code":-1,"error":0,"method":0,"result":{"serverHostVersion":"5.7.106","serverVersion":"4.0.18","state":6}}"#
        );
    }

    #[test]
    fn the_all_devices_mask_is_minus_one_and_masks_beyond_an_int_match_nothing() {
        let parse = |mask: &str| {
            request(&format!(r#"{{"method":1,"code":1,"params":{{"deviceTypeMask":{mask},"sizeMax":64}}}}"#))
        };
        assert_eq!(parse("-1").unwrap(), Request::GetDevices { type_mask: device_type::ALL, size_max: 64 });
        for mask in ["4294967295", "4294967295.0", "2147483648", "-2147483649"] {
            assert_eq!(parse(mask).unwrap(), Request::GetDevices { type_mask: 0, size_max: 64 }, "{mask}");
        }
        for mask in ["1.5", r#""1""#, "null"] {
            assert!(
                matches!(parse(mask), Err(RequestError::BadParams(key)) if key.contains("deviceTypeMask")),
                "{mask}"
            );
        }
    }

    #[test]
    fn u32_fields_reject_values_outside_the_signed_and_unsigned_range() {
        for luid in ["4294967296", "-2147483649", "1.5", r#""1""#, "null"] {
            let text = format!(r#"{{"method":8,"code":1,"params":{{"deviceId":"d","luids":[{luid}]}}}}"#);
            assert!(matches!(request(&text), Err(RequestError::BadParams(key)) if key.contains("luids[0]")), "{luid}");
        }
    }

    #[test]
    fn led_colors_unpack_argb_values_written_either_way() {
        for value in ["4279246896", "-15720400"] {
            let text = format!(
                r#"{{"method":10,"code":2,"params":{{"deviceId":"d","ledColors":[{{"id":42,"value":{value}}}]}}}}"#
            );
            let Request::SetLedColors { colors, .. } = request(&text).unwrap() else { panic!("expected SetLedColors") };
            assert_eq!(colors, [LedColor { luid: Luid(42), rgb: Rgb::new(0x10, 0x20, 0x30), alpha: 0xFF }], "{value}");
            assert_eq!(colors[0].value(), 0xFF10_2030);
        }
    }

    #[test]
    fn a_bad_led_color_names_its_position() {
        let text =
            r#"{"method":10,"code":2,"params":{"deviceId":"d","ledColors":[{"id":1,"value":0},{"id":"x","value":0}]}}"#;
        assert!(matches!(request(text), Err(RequestError::BadParams(key)) if key.contains("ledColors[1].id")));
    }

    #[test]
    fn key_names_are_accepted_as_a_character_code_only() {
        let parse =
            |key: &str| request(&format!(r#"{{"method":6,"code":3,"params":{{"deviceId":"d","keyName":{key}}}}}"#));
        assert_eq!(parse("65").unwrap(), Request::LuidForKeyName { device_id: "d".into(), key: 'A' });
        assert!(matches!(parse(r#""A""#), Err(RequestError::BadParams(_))));
        assert!(matches!(parse(r#""AB""#), Err(RequestError::BadParams(_))));
        assert!(matches!(parse(r#""""#), Err(RequestError::BadParams(_))));
        assert!(matches!(parse("128"), Err(RequestError::BadParams(_))));
        assert!(matches!(parse("-1"), Err(RequestError::BadParams(_))));
    }

    #[test]
    fn unknown_methods_bad_params_and_non_json_are_told_apart() {
        let unknown = parse_envelope(br#"{"method":42,"code":1}"#).unwrap();
        assert_eq!(parse_request(&unknown), Err(RequestError::UnknownMethod(42)));
        let event = parse_envelope(br#"{"method":17,"code":1}"#).unwrap();
        assert_eq!(parse_request(&event), Err(RequestError::UnknownMethod(17)));
        let missing = parse_envelope(br#"{"method":2,"code":1,"params":{}}"#).unwrap();
        assert!(matches!(parse_request(&missing), Err(RequestError::BadParams(key)) if key.contains("deviceId")));
        assert!(matches!(parse_envelope(b"{nope"), Err(EnvelopeError::NotJson(_))));
        assert_eq!(parse_envelope(br#"{"code":1}"#), Err(EnvelopeError::NoMethod));
        assert_eq!(parse_envelope(br#"[1]"#), Err(EnvelopeError::NoMethod));
        assert_eq!(parse_envelope(br#"{"method":"0"}"#), Err(EnvelopeError::NoMethod));
    }

    #[test]
    fn the_envelope_defaults_code_and_params() {
        let envelope = parse_envelope(br#"{"method":14}"#).unwrap();
        assert_eq!((envelope.code, envelope.params.is_empty()), (0, true));
    }

    #[test]
    fn index_defaults_to_zero() {
        let text = r#"{"method":4,"code":1,"params":{"deviceId":"d","propertyId":8}}"#;
        assert_eq!(request(text).unwrap(), Request::ReadProperty { device_id: "d".into(), property: 8, index: 0 });
    }

    #[test]
    fn every_method_id_parses_its_documented_params() {
        let color = LedColor { luid: Luid(1), rgb: Rgb::new(0, 0, 0), alpha: 0 };
        let device = || "d".to_string();
        let table: Vec<(i64, &str, Request)> = vec![
            (
                0,
                r#"{"pid":7,"clientVersion":"4.0.12"}"#,
                Request::Handshake { pid: Some(7), client_version: Some("4.0.12".into()) },
            ),
            (1, r#"{"deviceTypeMask":1,"sizeMax":64}"#, Request::GetDevices { type_mask: 1, size_max: 64 }),
            (2, r#"{"deviceId":"d"}"#, Request::GetDeviceInfo { device_id: device() }),
            (
                3,
                r#"{"deviceId":"d","propertyId":8,"index":2}"#,
                Request::GetPropertyInfo { device_id: device(), property: 8, index: 2 },
            ),
            (
                4,
                r#"{"deviceId":"d","propertyId":8,"index":0}"#,
                Request::ReadProperty { device_id: device(), property: 8, index: 0 },
            ),
            (
                5,
                r#"{"deviceId":"d","propertyId":2,"index":0,"property":{"dataType":0,"value":true}}"#,
                Request::WriteProperty { device_id: device(), property: 2, index: 0 },
            ),
            (6, r#"{"deviceId":"d","keyName":65}"#, Request::LuidForKeyName { device_id: device(), key: 'A' }),
            (7, r#"{"deviceId":"d","sizeMax":512}"#, Request::GetLedPositions { device_id: device(), size_max: 512 }),
            (
                8,
                r#"{"deviceId":"d","luids":[1,65537]}"#,
                Request::GetLedColors { device_id: device(), luids: vec![Luid(1), Luid(65537)] },
            ),
            (9, r#"{"priority":128}"#, Request::SetLayerPriority { priority: 128 }),
            (
                10,
                r#"{"deviceId":"d","ledColors":[{"id":1,"value":0}]}"#,
                Request::SetLedColors { device_id: device(), colors: vec![color] },
            ),
            (
                11,
                r#"{"devices":[{"deviceId":"d","ledColors":[{"id":1,"value":0}]}]}"#,
                Request::FlushLedColors { devices: vec![(device(), vec![color])] },
            ),
            (12, r#"{"deviceId":"d","accessLevel":3}"#, Request::RequestControl { device_id: device(), level: 3 }),
            (13, r#"{"deviceId":"d"}"#, Request::ReleaseControl { device_id: device() }),
            (14, "", Request::Subscribe),
            (15, "", Request::Unsubscribe),
            (
                16,
                r#"{"deviceId":"d","keyId":1,"isIntercepted":true}"#,
                Request::ConfigureKeyEvent { device_id: device(), key: 1, intercepted: true },
            ),
        ];
        for (method, params, expected) in table {
            let text = if params.is_empty() {
                format!(r#"{{"method":{method},"code":1}}"#)
            } else {
                format!(r#"{{"method":{method},"code":1,"params":{params}}}"#)
            };
            assert_eq!(request(&text).unwrap(), expected, "method {method}");
        }
    }

    #[test]
    fn a_flush_with_a_malformed_device_entry_is_bad_params() {
        let text = r#"{"method":11,"code":1,"params":{"devices":[{"ledColors":[]}]}}"#;
        assert!(matches!(request(text), Err(RequestError::BadParams(key)) if key.contains("devices[0].deviceId")));
    }

    #[test]
    fn events_have_the_method_17_shape() {
        assert_eq!(
            key_event("{d}", MacroKey(1), true),
            br#"{"code":-1,"error":0,"method":17,"result":{"id":2,"keyEvent":{"deviceId":"{d}","isPressed":true,"keyId":1}}}"#
        );
        assert_eq!(
            device_event("{d}", false),
            br#"{"code":-1,"error":0,"method":17,"result":{"deviceConnectionStatusChangedEvent":{"deviceId":"{d}","isConnected":false},"id":1}}"#
        );
    }

    #[test]
    fn integral_numbers_are_written_without_a_fraction() {
        assert_eq!(serde_json::to_string(&number(30.0)).unwrap(), "30");
        assert_eq!(serde_json::to_string(&number(30.5)).unwrap(), "30.5");
        assert_eq!(serde_json::to_string(&number(-4.0)).unwrap(), "-4");
    }
}
