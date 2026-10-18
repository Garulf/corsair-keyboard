use crate::ids::{
    DataType, ErrorCode, LedGroup, Luid, MacroKey, PropertyId, logical_layout, physical_layout, property_flag,
};
use k100::led;
use serde_json::{Value, json};
use std::sync::OnceLock;

pub const MODEL: &str = "K100 RGB";
pub const MM_PER_CANVAS_UNIT: f64 = 0.1905;

const KEYBOARD_DEVICE_TYPE: i64 = 1;
const MILLIMETRE_STEPS: f64 = 100.0;
const FIRST_LETTER_LED: u8 = 4;
const FIRST_DIGIT_LED: u8 = 30;
const ZERO_LED: u8 = 39;

#[rustfmt::skip]
const KEYBOARD_LEDS: [(u16, u8); 110] = [
    (1, 41), (2, 58), (3, 59), (4, 60), (5, 61), (6, 62), (7, 63), (8, 64), (9, 65), (10, 66), (11, 67), (12, 68), (13, 69),
    (14, 53), (15, 30), (16, 31), (17, 32), (18, 33), (19, 34), (20, 35), (21, 36), (22, 37), (23, 38), (24, 39), (25, 45),
    (26, 46), (27, 42), (28, 43), (29, 20), (30, 26), (31, 8), (32, 21), (33, 23), (34, 28), (35, 24), (36, 12), (37, 18),
    (38, 19), (39, 47), (40, 48), (41, 57), (42, 4), (43, 22), (44, 7), (45, 9), (46, 10), (47, 11), (48, 13), (49, 14),
    (50, 15), (51, 51), (52, 52), (53, 49), (54, 40), (55, 106), (57, 29), (58, 27), (59, 6), (60, 25), (61, 5), (62, 17),
    (63, 16), (64, 54), (65, 55), (66, 56), (67, 110), (68, 105), (69, 108), (70, 107), (71, 44), (72, 111), (74, 101),
    (75, 109), (84, 70), (85, 71), (86, 72), (87, 73), (88, 74), (89, 75), (90, 76), (91, 77), (92, 78), (93, 82), (94, 80),
    (95, 81), (96, 79), (99, 114), (100, 102), (101, 123), (102, 126), (103, 124), (104, 125), (105, 83), (106, 84),
    (107, 85), (108, 86), (109, 95), (110, 96), (111, 97), (112, 87), (113, 92), (114, 93), (115, 94), (116, 89), (117, 90),
    (118, 91), (120, 88), (121, 98), (122, 99), (129, 122),
];

const G_KEY_COUNT: u16 = 6;
const EDGE_ZONE_COUNT: u16 = 45;
const OEM_COUNT: u16 = 12;
const SHARED_EDGE_LED: u8 = 145;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SdkLed {
    pub luid: Luid,
    pub led: u8,
    pub cx: f64,
    pub cy: f64,
}

fn g_key_led(index: u16) -> u8 {
    led::G1 + (index - 1) as u8
}

fn edge_led(zone: u16) -> u8 {
    match zone {
        1..=11 => 171 - zone as u8,
        12..=18 => 126 + zone as u8,
        19 | 20 => SHARED_EDGE_LED,
        21..=34 => 125 + zone as u8,
        _ => 136 + zone as u8,
    }
}

fn oem_led(index: u16) -> u8 {
    match index {
        1..=3 => led::LOGO_FIRST + (index - 1) as u8,
        4 => led::DIAL_KEY,
        _ => 177 + index as u8,
    }
}

fn luid_table() -> Vec<(Luid, u8)> {
    let keyboard = KEYBOARD_LEDS.iter().map(|&(id, led)| (Luid::new(LedGroup::Keyboard, id), led));
    let g_keys = (1..=G_KEY_COUNT).map(|i| (Luid::new(LedGroup::GKeys, i), g_key_led(i)));
    let edge = (1..=EDGE_ZONE_COUNT).map(|z| (Luid::new(LedGroup::Edge, z), edge_led(z)));
    let oem = (1..=OEM_COUNT).map(|i| (Luid::new(LedGroup::Oem, i), oem_led(i)));
    keyboard.chain(g_keys).chain(edge).chain(oem).collect()
}

fn millimetres(canvas_units: f32) -> f64 {
    (canvas_units as f64 * MM_PER_CANVAS_UNIT * MILLIMETRE_STEPS).round() / MILLIMETRE_STEPS
}

pub fn leds() -> &'static [SdkLed] {
    static LEDS: OnceLock<Vec<SdkLed>> = OnceLock::new();
    LEDS.get_or_init(|| {
        let layout = engine::Layout::ansi();
        luid_table()
            .into_iter()
            .map(|(luid, led)| {
                let (x, y) = layout.get(led).map_or((0.0, 0.0), |geometry| geometry.rect.center());
                SdkLed { luid, led, cx: millimetres(x), cy: millimetres(y) }
            })
            .collect()
    })
}

pub fn led_for(luid: Luid) -> Option<u8> {
    leds().iter().find(|entry| entry.luid == luid).map(|entry| entry.led)
}

fn luid_of_led(led: u8) -> Option<Luid> {
    let keyboard = leds().iter().filter(|entry| entry.luid.group() == LedGroup::Keyboard as u16);
    keyboard.clone().find(|entry| entry.led == led).map(|entry| entry.luid)
}

pub fn luid_for_key_name(key: char) -> Option<Luid> {
    let led = match key.to_ascii_lowercase() {
        letter @ 'a'..='z' => FIRST_LETTER_LED + (letter as u8 - b'a'),
        '0' => ZERO_LED,
        digit @ '1'..='9' => FIRST_DIGIT_LED + (digit as u8 - b'1'),
        _ => return None,
    };
    luid_of_led(led)
}

pub fn macro_key(led: u8) -> Option<MacroKey> {
    (led::G1..=led::G6).contains(&led).then(|| MacroKey(led - led::G1 + 1))
}

pub fn macro_key_led(key: MacroKey) -> Option<u8> {
    (1..=G_KEY_COUNT as u8).contains(&key.0).then(|| led::G1 + key.0 - 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub id: String,
    pub serial: String,
    pub physical_layout: i32,
    pub logical_layout: i32,
}

pub fn device_info(device: &DeviceSnapshot) -> Value {
    json!({
        "channelCount": 0,
        "id": device.id,
        "ledCount": leds().len(),
        "model": MODEL,
        "serial": device.serial,
        "type": KEYBOARD_DEVICE_TYPE,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Int32(i32),
    Int32Array(Vec<i32>),
}

impl PropertyValue {
    pub fn data_type(&self) -> DataType {
        match self {
            PropertyValue::Int32(_) => DataType::Int32,
            PropertyValue::Int32Array(_) => DataType::Int32Array,
        }
    }

    pub fn to_json(&self) -> Value {
        let value = match self {
            PropertyValue::Int32(value) => json!(value),
            PropertyValue::Int32Array(values) => json!(values),
        };
        json!({"dataType": self.data_type().id(), "value": value})
    }
}

fn readable_property(property: i64) -> Result<PropertyId, ErrorCode> {
    let id = PropertyId::from_id(property).ok_or(ErrorCode::InvalidArguments)?;
    match id {
        PropertyId::PropertyArray
        | PropertyId::PhysicalLayout
        | PropertyId::LogicalLayout
        | PropertyId::MacroKeyArray => Ok(id),
        _ => Err(ErrorCode::InvalidOperation),
    }
}

pub fn property_flags(property: i64) -> Result<u32, ErrorCode> {
    readable_property(property).map(|_| property_flag::CAN_READ)
}

pub fn read_property(device: &DeviceSnapshot, property: i64) -> Result<PropertyValue, ErrorCode> {
    Ok(match readable_property(property)? {
        PropertyId::PropertyArray => {
            let supported = [PropertyId::PhysicalLayout, PropertyId::LogicalLayout, PropertyId::MacroKeyArray];
            PropertyValue::Int32Array(supported.iter().map(|id| id.id() as i32).collect())
        }
        PropertyId::PhysicalLayout => PropertyValue::Int32(device.physical_layout),
        PropertyId::LogicalLayout => PropertyValue::Int32(device.logical_layout),
        _ => PropertyValue::Int32Array((1..=G_KEY_COUNT as i32).collect()),
    })
}

pub fn layouts_for(keyboard_layout: u32) -> (i32, i32) {
    match keyboard_layout {
        2 => (physical_layout::UK, logical_layout::EU),
        3 => (physical_layout::BR, logical_layout::BR),
        4 => (physical_layout::JP, logical_layout::JP),
        _ => (physical_layout::US, logical_layout::NA),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_k100_exposes_173_luids_on_172_leds() {
        let count = |group| leds().iter().filter(|led| led.luid.group() == group).count();
        assert_eq!((count(0), count(1), count(2), count(3)), (110, 6, 45, 12));
        let distinct: std::collections::BTreeSet<u8> = leds().iter().map(|led| led.led).collect();
        assert_eq!((leds().len(), distinct.len()), (173, 172));
    }

    #[test]
    fn luids_map_to_the_k100_led_indices() {
        for (luid, led) in [
            (0x0000_0001, 41),
            (0x0000_001E, 26),
            (0x0000_0081, 122),
            (0x0001_0001, 131),
            (0x0001_0006, 136),
            (0x0002_0001, 170),
            (0x0002_000B, 160),
            (0x0002_0013, 145),
            (0x0002_0014, 145),
            (0x0002_0022, 159),
            (0x0002_002D, 181),
            (0x0003_0001, 190),
            (0x0003_0004, 137),
            (0x0003_000C, 189),
        ] {
            assert_eq!(led_for(Luid(luid)), Some(led), "{luid:#x}");
        }
        assert_eq!(led_for(Luid(0x0001_0007)), None);
        assert!(leds().iter().all(|led| led.led != k100::led::PROFILE));
    }

    #[test]
    fn luids_come_in_group_then_index_order() {
        assert!(leds().windows(2).all(|pair| pair[0].luid < pair[1].luid));
    }

    #[test]
    fn every_led_has_a_position_in_millimetres() {
        assert!(
            leds()
                .iter()
                .all(|led| led.cx.is_finite() && (0.0..=500.0).contains(&led.cx) && (0.0..=200.0).contains(&led.cy))
        );
        let at = |luid| leds().iter().find(|led| led.luid == Luid(luid)).unwrap();
        assert!(at(1).cx < at(2).cx, "Escape left of F1");
        assert!(at(0x0001_0001).cy < at(0x0001_0006).cy, "G1 above G6");
    }

    #[test]
    fn every_led_has_geometry_in_the_ansi_layout() {
        let layout = engine::Layout::ansi();
        assert!(leds().iter().all(|entry| layout.get(entry.led).is_some()));
    }

    #[test]
    fn positions_are_the_layout_centres_in_millimetres_to_the_hundredth() {
        let space = leds().iter().find(|led| led.luid == Luid(0x0000_0047)).unwrap();
        assert_eq!(space.led, 44);
        let (x, y) = engine::Layout::ansi().get(44).unwrap().rect.center();
        let hundredths = |canvas: f32| (canvas as f64 * MM_PER_CANVAS_UNIT * 100.0).round();
        assert_eq!((space.cx * 100.0).round(), hundredths(x));
        assert_eq!((space.cy * 100.0).round(), hundredths(y));
    }

    #[test]
    fn key_names_are_letters_and_digits() {
        assert_eq!(luid_for_key_name('a'), Some(Luid(42)));
        assert_eq!(luid_for_key_name('A'), Some(Luid(42)));
        assert_eq!(luid_for_key_name('w'), Some(Luid(30)));
        assert_eq!((luid_for_key_name('1'), luid_for_key_name('0')), (Some(Luid(15)), Some(Luid(24))));
        assert_eq!(luid_for_key_name(';'), None);
        assert_eq!(luid_for_key_name('\u{e9}'), None);
    }

    #[test]
    fn every_letter_and_digit_has_a_luid() {
        for key in ('a'..='z').chain('0'..='9') {
            assert!(luid_for_key_name(key).is_some(), "{key}");
        }
    }

    #[test]
    fn the_macro_keys_are_g1_to_g6() {
        assert_eq!(macro_key(131), Some(MacroKey(1)));
        assert_eq!(macro_key(137), None);
        assert_eq!(macro_key_led(MacroKey(6)), Some(136));
        assert_eq!(macro_key_led(MacroKey(7)), None);
        assert_eq!(macro_key_led(MacroKey(0)), None);
    }

    #[test]
    fn the_four_keyboard_properties_read_back() {
        let device = DeviceSnapshot { id: "{d}".into(), serial: "S".into(), physical_layout: 1, logical_layout: 2 };
        assert_eq!(read_property(&device, 8), Ok(PropertyValue::Int32Array(vec![1, 2, 3, 4, 5, 6])));
        assert_eq!(read_property(&device, 8).unwrap().to_json(), json!({"dataType": 17, "value": [1, 2, 3, 4, 5, 6]}));
        assert_eq!(read_property(&device, 6), Ok(PropertyValue::Int32(1)));
        assert_eq!(read_property(&device, 7), Ok(PropertyValue::Int32(2)));
        assert_eq!(read_property(&device, 1), Ok(PropertyValue::Int32Array(vec![6, 7, 8])));
        assert_eq!(read_property(&device, 6).unwrap().to_json(), json!({"dataType": 1, "value": 1}));
        assert_eq!(property_flags(1), Ok(property_flag::CAN_READ));
        assert_eq!(property_flags(8), Ok(property_flag::CAN_READ));
        assert_eq!(property_flags(2), Err(ErrorCode::InvalidOperation));
        assert_eq!(read_property(&device, 2), Err(ErrorCode::InvalidOperation));
        assert_eq!(read_property(&device, 99), Err(ErrorCode::InvalidArguments));
        assert_eq!(property_flags(0), Err(ErrorCode::InvalidArguments));
    }

    #[test]
    fn the_device_info_names_the_k100() {
        let device = DeviceSnapshot { id: "{d}".into(), serial: "S".into(), physical_layout: 1, logical_layout: 2 };
        assert_eq!(
            device_info(&device),
            json!({"channelCount": 0, "id": "{d}", "ledCount": 173, "model": "K100 RGB", "serial": "S", "type": 1})
        );
        assert_eq!(layouts_for(1), (physical_layout::US, logical_layout::NA));
        assert_eq!(layouts_for(4), (physical_layout::JP, logical_layout::JP));
        assert_eq!(layouts_for(2), (physical_layout::UK, logical_layout::EU));
        assert_eq!(layouts_for(3), (physical_layout::BR, logical_layout::BR));
        assert_eq!(layouts_for(0), (physical_layout::US, logical_layout::NA));
        assert_eq!(layouts_for(99), (physical_layout::US, logical_layout::NA));
    }
}
