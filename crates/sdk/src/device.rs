use crate::ids::{
    DataType, ErrorCode, LedGroup, Luid, MacroKey, PropertyId, logical_layout, physical_layout, property_flag,
};
use k100::led;
use serde_json::{Value, json};
use std::sync::OnceLock;

pub const MODEL: &str = "K100 RGB";

const KEYBOARD_DEVICE_TYPE: i64 = 1;
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

#[rustfmt::skip]
const POSITIONS: [(u32, f64, f64); 173] = [
    (131099, 498.0, 71.0), (131100, 498.0, 86.0), (108, 481.0, 73.0), (55, 60.0, 135.0), (31, 117.0, 94.0),
    (65538, 18.0, 73.0), (66, 274.0, 135.0), (93, 375.0, 135.0), (131079, 151.0, 3.4000000000000004),
    (196614, 64.0, 7.0), (60, 153.0, 135.0), (28, 52.0, 94.0), (51, 264.0, 114.0), (131085, 283.0, 3.4000000000000004),
    (131115, 3.6, 148.0), (12, 309.0, 53.0), (23, 228.0, 73.0), (38, 258.0, 94.0), (100, 421.0, 18.0),
    (104, 480.0, 46.0), (61, 173.0, 135.0), (115, 461.0, 114.0), (42, 83.0, 114.0), (19, 148.0, 73.0),
    (59, 133.0, 135.0), (67, 313.0, 133.0), (27, 319.0, 73.0), (131116, 3.6, 163.0), (36, 218.0, 94.0),
    (106, 441.0, 73.0), (131104, 498.0, 148.0), (196617, 72.0, 27.0), (131110, 3.6, 71.0), (116, 421.0, 135.0),
    (1, 47.0, 53.0), (65, 254.0, 135.0), (131083, 239.0, 3.4000000000000004), (52, 284.0, 114.0),
    (131078, 129.0, 3.4000000000000004), (131091, 417.0, 3.4000000000000004), (41, 55.0, 114.0), (91, 375.0, 94.0),
    (131109, 3.6, 55.0), (25, 268.0, 73.0), (75, 327.0, 156.0), (131097, 498.0, 40.0),
    (131073, 17.0, 3.4000000000000004), (5, 140.0, 53.0), (96, 395.0, 156.0), (131089, 373.0, 3.4000000000000004),
    (131103, 498.0, 132.0), (72, 251.0, 156.0), (16, 87.0, 73.0), (196619, 56.0, 26.6),
    (131080, 173.0, 3.4000000000000004), (131093, 463.0, 3.4000000000000004), (17, 107.0, 73.0), (58, 113.0, 135.0),
    (9, 235.0, 53.0), (8, 215.0, 53.0), (26, 288.0, 73.0), (40, 299.0, 94.0), (14, 47.0, 74.0), (43, 103.0, 114.0),
    (131077, 107.0, 3.4000000000000004), (118, 461.0, 135.0), (49, 223.0, 114.0), (131102, 498.0, 117.0),
    (34, 178.0, 94.0), (113, 421.0, 114.0), (131101, 498.0, 102.0), (89, 395.0, 73.0), (95, 375.0, 156.0),
    (65541, 18.0, 135.0), (122, 461.0, 156.0), (103, 461.0, 46.0), (29, 77.0, 94.0), (110, 441.0, 94.0),
    (88, 375.0, 73.0), (131095, 498.0, 9.4), (102, 441.0, 46.0), (131105, 498.0, 163.0), (196610, 64.0, 18.0),
    (196613, 273.0, 16.0), (48, 203.0, 114.0), (129, 276.0, 156.0), (64, 234.0, 135.0), (196612, 249.0, 16.0),
    (10, 269.0, 53.0), (65542, 18.0, 156.0), (131082, 218.0, 3.4000000000000004), (131084, 261.0, 3.4000000000000004),
    (65540, 18.0, 114.0), (131096, 498.0, 24.4), (131094, 486.0, 3.4000000000000004), (37, 239.0, 94.0),
    (131106, 3.6, 9.4), (131075, 62.0, 3.4000000000000004), (131107, 3.6, 24.4), (13, 329.0, 53.0), (22, 208.0, 73.0),
    (92, 395.0, 94.0), (196616, 75.0, 19.0), (131111, 3.6, 86.0), (47, 183.0, 114.0), (7, 195.0, 53.0),
    (131081, 196.0, 3.4000000000000004), (196615, 73.0, 11.0), (99, 90.0, 18.0), (11, 289.0, 53.0),
    (196620, 52.400000000000006, 19.0), (74, 301.0, 156.0), (45, 143.0, 114.0), (131098, 498.0, 55.0),
    (46, 163.0, 114.0), (63, 213.0, 135.0), (35, 198.0, 94.0), (131112, 3.6, 102.0), (44, 123.0, 114.0),
    (2, 80.0, 53.0), (68, 50.0, 156.0), (196611, 226.0, 16.0), (117, 441.0, 135.0), (70, 100.0, 156.0),
    (69, 75.0, 156.0), (196618, 64.0, 30.0), (54, 317.0, 114.0), (196621, 55.2, 10.8), (196609, 38.0, 18.0),
    (6, 175.0, 53.0), (30, 97.0, 94.0), (120, 483.0, 145.0), (131087, 329.0, 3.4000000000000004),
    (131086, 305.0, 3.4000000000000004), (65539, 18.0, 94.0), (131074, 39.0, 3.4000000000000004), (4, 120.0, 53.0),
    (109, 421.0, 94.0), (32, 137.0, 94.0), (53, 325.0, 94.0), (131114, 3.6, 132.0), (111, 461.0, 94.0),
    (131113, 3.6, 117.0), (18, 127.0, 73.0), (131092, 440.0, 3.4000000000000004), (114, 441.0, 114.0),
    (121, 431.0, 156.0), (112, 483.0, 104.0), (94, 355.0, 156.0), (3, 100.0, 53.0), (21, 187.0, 73.0),
    (85, 375.0, 53.0), (105, 421.0, 73.0), (84, 355.0, 53.0), (86, 395.0, 53.0), (20, 167.0, 73.0), (57, 93.0, 135.0),
    (24, 248.0, 73.0), (15, 67.0, 73.0), (87, 355.0, 73.0), (131108, 3.6, 40.0), (90, 355.0, 94.0),
    (131088, 351.0, 3.4000000000000004), (50, 243.0, 114.0), (33, 158.0, 94.0), (39, 279.0, 94.0),
    (131076, 85.0, 3.4000000000000004), (107, 461.0, 73.0), (131090, 395.0, 3.4000000000000004), (65537, 18.0, 53.0),
    (101, 421.0, 46.0), (62, 183.0, 135.0), (71, 176.0, 156.0),
];

const G_KEY_COUNT: u16 = 6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SdkLed {
    pub luid: Luid,
    pub led: u8,
    pub cx: f64,
    pub cy: f64,
}

fn keyboard_led(id: u16) -> Option<u8> {
    KEYBOARD_LEDS.iter().find(|&&(luid, _)| luid == id).map(|&(_, led)| led)
}

fn g_key_led(index: u16) -> Option<u8> {
    (1..=G_KEY_COUNT).contains(&index).then(|| led::G1 + (index - 1) as u8)
}

fn edge_led(zone: u16) -> Option<u8> {
    let zone = u8::try_from(zone).ok()?;
    match zone {
        1..=22 => Some(led::LIGHT_BAR_FIRST + zone - 1),
        23..=33 => Some(led::RIGHT_UNDERGLOW_FIRST + zone - 23),
        34..=44 => Some(led::LEFT_UNDERGLOW_FIRST + zone - 34),
        _ => None,
    }
}

fn oem_led(index: u16) -> Option<u8> {
    let index = u8::try_from(index).ok()?;
    match index {
        1 => Some(led::PROFILE),
        2 => Some(led::DIAL_KEY),
        3..=5 => Some(led::LOGO_FIRST + index - 3),
        6..=13 => Some(led::DIAL_RING_FIRST + index - 6),
        _ => None,
    }
}

pub fn led_for(luid: Luid) -> Option<u8> {
    const KEYBOARD: u16 = LedGroup::Keyboard as u16;
    const G_KEYS: u16 = LedGroup::GKeys as u16;
    const EDGE: u16 = LedGroup::Edge as u16;
    const OEM: u16 = LedGroup::Oem as u16;
    match luid.group() {
        KEYBOARD => keyboard_led(luid.index()),
        G_KEYS => g_key_led(luid.index()),
        EDGE => edge_led(luid.index()),
        OEM => oem_led(luid.index()),
        _ => None,
    }
}

pub fn leds() -> &'static [SdkLed] {
    static LEDS: OnceLock<Vec<SdkLed>> = OnceLock::new();
    LEDS.get_or_init(|| {
        POSITIONS
            .iter()
            .map(|&(id, cx, cy)| {
                let luid = Luid(id);
                let led = led_for(luid).unwrap_or_else(|| panic!("LUID {id:#x} has no K100 LED"));
                SdkLed { luid, led, cx, cy }
            })
            .collect()
    })
}

fn luid_of_key(led: u8) -> Option<Luid> {
    KEYBOARD_LEDS.iter().find(|&&(_, key)| key == led).map(|&(id, _)| Luid::new(LedGroup::Keyboard, id))
}

pub fn luid_for_key_name(key: char) -> Option<Luid> {
    let led = match key.to_ascii_lowercase() {
        letter @ 'a'..='z' => FIRST_LETTER_LED + (letter as u8 - b'a'),
        '0' => ZERO_LED,
        digit @ '1'..='9' => FIRST_DIGIT_LED + (digit as u8 - b'1'),
        _ => return None,
    };
    luid_of_key(led)
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
    let id = PropertyId::from_id(property).ok_or(ErrorCode::InvalidOperation)?;
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
            let supported = [
                PropertyId::PropertyArray,
                PropertyId::PhysicalLayout,
                PropertyId::LogicalLayout,
                PropertyId::MacroKeyArray,
            ];
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
    use std::f64::consts::TAU;

    fn luids(group: LedGroup, indices: std::ops::RangeInclusive<u16>) -> Vec<Option<u8>> {
        indices.map(|index| led_for(Luid::new(group, index))).collect()
    }

    fn some(leds: std::ops::RangeInclusive<u8>) -> Vec<Option<u8>> {
        leds.map(Some).collect()
    }

    fn position(luid: Luid) -> (f64, f64) {
        let entry = leds().iter().find(|entry| entry.luid == luid).unwrap();
        (entry.cx, entry.cy)
    }

    #[test]
    fn the_k100_exposes_icues_173_luids_on_173_leds() {
        let count = |group| leds().iter().filter(|led| led.luid.group() == group).count();
        assert_eq!((count(0), count(1), count(2), count(3)), (110, 6, 44, 13));
        let distinct: std::collections::BTreeSet<u8> = leds().iter().map(|led| led.led).collect();
        assert_eq!((leds().len(), distinct.len()), (173, 173));
    }

    #[test]
    fn keyboard_and_g_key_luids_map_to_the_k100_led_indices() {
        for (luid, led) in [(0x0000_0001, 41), (0x0000_001E, 26), (0x0000_0035, 49), (0x0000_0081, 122)] {
            assert_eq!(led_for(Luid(luid)), Some(led), "{luid:#x}");
        }
        assert_eq!(luids(LedGroup::GKeys, 1..=7), [some(131..=136), vec![None]].concat());
        assert_eq!((led_for(Luid(97)), led_for(Luid(98))), (None, None), "Profile and Brightness have no CLK LUID");
    }

    #[test]
    fn edge_zones_map_to_the_light_bar_then_the_right_and_left_underglow() {
        assert_eq!(luids(LedGroup::Edge, 1..=22), some(led::LIGHT_BAR_FIRST..=led::LIGHT_BAR_LAST));
        assert_eq!(luids(LedGroup::Edge, 23..=33), some(led::RIGHT_UNDERGLOW_FIRST..=led::RIGHT_UNDERGLOW_LAST));
        assert_eq!(luids(LedGroup::Edge, 34..=44), some(led::LEFT_UNDERGLOW_FIRST..=led::LEFT_UNDERGLOW_LAST));
        assert_eq!((led_for(Luid(0x0002_0000)), led_for(Luid(0x0002_002D))), (None, None));
    }

    #[test]
    fn oem_luids_map_to_profile_dial_logo_and_ring() {
        let expected = [vec![Some(led::PROFILE), Some(led::DIAL_KEY)], some(led::LOGO_FIRST..=led::LOGO_LAST)].concat();
        assert_eq!(luids(LedGroup::Oem, 1..=5), expected);
        assert_eq!(luids(LedGroup::Oem, 6..=13), some(led::DIAL_RING_FIRST..=led::DIAL_RING_LAST));
        assert_eq!((led_for(Luid(0x0003_0000)), led_for(Luid(0x0003_000E))), (None, None));
    }

    fn icue_centre(led: u8) -> (f64, f64) {
        let entry = leds().iter().find(|entry| entry.led == led).unwrap();
        (entry.cx, entry.cy)
    }

    fn layout_centre(layout: &engine::Layout, led: u8) -> (f64, f64) {
        let (x, y) = layout.get(led).unwrap().rect.center();
        (x as f64, y as f64)
    }

    #[test]
    fn icue_positions_order_each_strip_like_the_layout() {
        let layout = engine::Layout::ansi();
        let along = |horizontal: bool, (x, y): (f64, f64)| if horizontal { x } else { y };
        for (strip, horizontal) in [
            (led::LIGHT_BAR_FIRST..=led::LIGHT_BAR_LAST, true),
            (led::LEFT_UNDERGLOW_FIRST..=led::LEFT_UNDERGLOW_LAST, false),
            (led::RIGHT_UNDERGLOW_FIRST..=led::RIGHT_UNDERGLOW_LAST, false),
            (led::LOGO_FIRST..=led::LOGO_LAST, true),
        ] {
            let ordered_by = |centre: &dyn Fn(u8) -> (f64, f64)| {
                let mut members: Vec<u8> = strip.clone().collect();
                members.sort_by(|&a, &b| along(horizontal, centre(a)).total_cmp(&along(horizontal, centre(b))));
                members
            };
            assert_eq!(ordered_by(&icue_centre), ordered_by(&|led| layout_centre(&layout, led)), "{strip:?}");
        }
    }

    #[test]
    fn icue_places_the_edge_strips_around_the_keys() {
        let keys = || leds().iter().filter(|entry| entry.luid.group() == 0);
        let edge = |strip: std::ops::RangeInclusive<u8>| leds().iter().filter(move |entry| strip.contains(&entry.led));
        let top = keys().map(|key| key.cy).fold(f64::MAX, f64::min);
        let (left, right) = keys().fold((f64::MAX, f64::MIN), |(l, r), key| (l.min(key.cx), r.max(key.cx)));
        assert!(edge(led::LIGHT_BAR_FIRST..=led::LIGHT_BAR_LAST).all(|zone| zone.cy < top));
        assert!(edge(led::LEFT_UNDERGLOW_FIRST..=led::LEFT_UNDERGLOW_LAST).all(|zone| zone.cx < left));
        assert!(edge(led::RIGHT_UNDERGLOW_FIRST..=led::RIGHT_UNDERGLOW_LAST).all(|zone| zone.cx > right));
    }

    #[test]
    fn icue_rings_the_dial_key_clockwise_from_the_top_like_the_layout() {
        let layout = engine::Layout::ansi();
        let clockwise_from_top = |(x, y): (f64, f64), (cx, cy): (f64, f64)| (x - cx).atan2(cy - y).rem_euclid(TAU);
        let (icue_dial, layout_dial) = (icue_centre(led::DIAL_KEY), layout_centre(&layout, led::DIAL_KEY));
        for ring in led::DIAL_RING_FIRST..=led::DIAL_RING_LAST {
            let expected = clockwise_from_top(layout_centre(&layout, ring), layout_dial);
            let actual = clockwise_from_top(icue_centre(ring), icue_dial);
            let off = (actual - expected).abs().min(TAU - (actual - expected).abs());
            assert!(off < TAU / 16.0, "LED {ring} is {off} rad from its layout angle");
        }
        let (icue_profile, layout_profile) = (icue_centre(led::PROFILE), layout_centre(&layout, led::PROFILE));
        assert!(
            icue_profile.0 < icue_dial.0 && icue_profile.1 == icue_dial.1,
            "Profile level with the dial, to its left"
        );
        assert!(layout_profile.0 < layout_dial.0);
    }

    #[test]
    fn luids_come_in_icues_order() {
        let ids: Vec<u32> = leds().iter().map(|led| led.luid.0).collect();
        assert_eq!(ids[..4], [131_099, 131_100, 108, 55]);
        assert_eq!(ids, POSITIONS.iter().map(|&(id, _, _)| id).collect::<Vec<_>>());
        assert_eq!(ids.iter().collect::<std::collections::BTreeSet<_>>().len(), ids.len());
    }

    #[test]
    fn every_led_has_a_position_in_millimetres() {
        assert!(
            leds()
                .iter()
                .all(|led| led.cx.is_finite() && (0.0..=500.0).contains(&led.cx) && (0.0..=200.0).contains(&led.cy))
        );
        let at = |luid| position(Luid(luid));
        assert!(at(1).0 < at(2).0, "Escape left of F1");
        assert!(at(0x0001_0001).1 < at(0x0001_0006).1, "G1 above G6");
    }

    #[test]
    fn every_led_has_geometry_in_the_ansi_layout() {
        let layout = engine::Layout::ansi();
        assert!(leds().iter().all(|entry| layout.get(entry.led).is_some()));
    }

    #[test]
    fn positions_are_icues_values_to_the_last_bit() {
        assert_eq!(position(Luid(1)), (47.0, 53.0));
        assert_eq!(position(Luid(0x0002_0001)), (17.0, 3.4000000000000004));
        assert_eq!(position(Luid(0x0003_000C)), (52.400000000000006, 19.0));
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
        assert_eq!(read_property(&device, 1), Ok(PropertyValue::Int32Array(vec![1, 6, 7, 8])));
        assert_eq!(read_property(&device, 6).unwrap().to_json(), json!({"dataType": 1, "value": 1}));
        assert_eq!(property_flags(1), Ok(property_flag::CAN_READ));
        assert_eq!(property_flags(8), Ok(property_flag::CAN_READ));
        assert_eq!(property_flags(2), Err(ErrorCode::InvalidOperation));
        assert_eq!(read_property(&device, 2), Err(ErrorCode::InvalidOperation));
        assert_eq!(read_property(&device, 99), Err(ErrorCode::InvalidOperation));
        assert_eq!(property_flags(0), Err(ErrorCode::InvalidOperation));
    }

    #[test]
    fn the_device_info_names_the_k100() {
        let device = DeviceSnapshot { id: "{d}".into(), serial: "S".into(), physical_layout: 1, logical_layout: 2 };
        assert_eq!(
            device_info(&device),
            json!({"channelCount": 0, "id": "{d}", "ledCount": 173, "model": "K100 RGB", "serial": "S", "type": 1})
        );
        assert_eq!(layouts_for(1), (physical_layout::US, logical_layout::NA));
        assert_eq!(layouts_for(1), (1, 2));
        assert_eq!(layouts_for(4), (physical_layout::JP, logical_layout::JP));
        assert_eq!(layouts_for(2), (physical_layout::UK, logical_layout::EU));
        assert_eq!(layouts_for(3), (physical_layout::BR, logical_layout::BR));
        assert_eq!(layouts_for(0), (physical_layout::US, logical_layout::NA));
        assert_eq!(layouts_for(99), (physical_layout::US, logical_layout::NA));
    }
}
