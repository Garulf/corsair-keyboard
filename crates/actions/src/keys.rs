use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Output {
    pub scan: u16,
    pub vk: u16,
    pub evdev: u16,
}

impl Output {
    pub fn is_extended(self) -> bool {
        self.scan & 0xFF00 == 0xE000
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct KeyDef {
    pub name: &'static str,
    pub index: Option<u8>,
    pub output: Option<Output>,
}

const MISSING_ON_K100: [u8; 2] = [113, 127];

impl KeyDef {
    pub fn is_on_k100(&self) -> bool {
        self.index.is_some_and(|index| !MISSING_ON_K100.contains(&index))
    }
}

const fn plain(name: &'static str, index: u8, scan: u16) -> KeyDef {
    KeyDef { name, index: Some(index), output: Some(Output { scan, vk: 0, evdev: scan }) }
}

const fn extended(name: &'static str, index: u8, scan: u16, evdev: u16) -> KeyDef {
    KeyDef { name, index: Some(index), output: Some(Output { scan: 0xE000 | scan, vk: 0, evdev }) }
}

const fn virtual_key(name: &'static str, index: u8, vk: u16, evdev: u16) -> KeyDef {
    KeyDef { name, index: Some(index), output: Some(Output { scan: 0, vk, evdev }) }
}

const fn output_only(name: &'static str, vk: u16, evdev: u16) -> KeyDef {
    KeyDef { name, index: None, output: Some(Output { scan: 0, vk, evdev }) }
}

const fn physical(name: &'static str, index: u8) -> KeyDef {
    KeyDef { name, index: Some(index), output: None }
}

static KEYS: &[KeyDef] = &[
    plain("a", 4, 0x1E),
    plain("b", 5, 0x30),
    plain("c", 6, 0x2E),
    plain("d", 7, 0x20),
    plain("e", 8, 0x12),
    plain("f", 9, 0x21),
    plain("g", 10, 0x22),
    plain("h", 11, 0x23),
    plain("i", 12, 0x17),
    plain("j", 13, 0x24),
    plain("k", 14, 0x25),
    plain("l", 15, 0x26),
    plain("m", 16, 0x32),
    plain("n", 17, 0x31),
    plain("o", 18, 0x18),
    plain("p", 19, 0x19),
    plain("q", 20, 0x10),
    plain("r", 21, 0x13),
    plain("s", 22, 0x1F),
    plain("t", 23, 0x14),
    plain("u", 24, 0x16),
    plain("v", 25, 0x2F),
    plain("w", 26, 0x11),
    plain("x", 27, 0x2D),
    plain("y", 28, 0x15),
    plain("z", 29, 0x2C),
    plain("1", 30, 0x02),
    plain("2", 31, 0x03),
    plain("3", 32, 0x04),
    plain("4", 33, 0x05),
    plain("5", 34, 0x06),
    plain("6", 35, 0x07),
    plain("7", 36, 0x08),
    plain("8", 37, 0x09),
    plain("9", 38, 0x0A),
    plain("0", 39, 0x0B),
    plain("enter", 40, 0x1C),
    plain("escape", 41, 0x01),
    plain("backspace", 42, 0x0E),
    plain("tab", 43, 0x0F),
    plain("space", 44, 0x39),
    plain("minus", 45, 0x0C),
    plain("equal", 46, 0x0D),
    plain("left_bracket", 47, 0x1A),
    plain("right_bracket", 48, 0x1B),
    plain("backslash", 49, 0x2B),
    plain("non_us_hash", 50, 0x2B),
    plain("semicolon", 51, 0x27),
    plain("apostrophe", 52, 0x28),
    plain("grave", 53, 0x29),
    plain("comma", 54, 0x33),
    plain("period", 55, 0x34),
    plain("slash", 56, 0x35),
    plain("caps_lock", 57, 0x3A),
    plain("f1", 58, 0x3B),
    plain("f2", 59, 0x3C),
    plain("f3", 60, 0x3D),
    plain("f4", 61, 0x3E),
    plain("f5", 62, 0x3F),
    plain("f6", 63, 0x40),
    plain("f7", 64, 0x41),
    plain("f8", 65, 0x42),
    plain("f9", 66, 0x43),
    plain("f10", 67, 0x44),
    plain("f11", 68, 0x57),
    plain("f12", 69, 0x58),
    extended("print_screen", 70, 0x37, 99),
    plain("scroll_lock", 71, 0x46),
    virtual_key("pause", 72, 0x13, 119),
    extended("insert", 73, 0x52, 110),
    extended("home", 74, 0x47, 102),
    extended("page_up", 75, 0x49, 104),
    extended("delete", 76, 0x53, 111),
    extended("end", 77, 0x4F, 107),
    extended("page_down", 78, 0x51, 109),
    extended("right", 79, 0x4D, 106),
    extended("left", 80, 0x4B, 105),
    extended("down", 81, 0x50, 108),
    extended("up", 82, 0x48, 103),
    virtual_key("num_lock", 83, 0x90, 69),
    extended("kp_divide", 84, 0x35, 98),
    plain("kp_multiply", 85, 0x37),
    plain("kp_minus", 86, 0x4A),
    plain("kp_plus", 87, 0x4E),
    extended("kp_enter", 88, 0x1C, 96),
    plain("kp_1", 89, 0x4F),
    plain("kp_2", 90, 0x50),
    plain("kp_3", 91, 0x51),
    plain("kp_4", 92, 0x4B),
    plain("kp_5", 93, 0x4C),
    plain("kp_6", 94, 0x4D),
    plain("kp_7", 95, 0x47),
    plain("kp_8", 96, 0x48),
    plain("kp_9", 97, 0x49),
    plain("kp_0", 98, 0x52),
    plain("kp_period", 99, 0x53),
    plain("non_us_backslash", 100, 0x56),
    extended("application", 101, 0x5D, 127),
    virtual_key("mute", 102, 0xAD, 113),
    virtual_key("volume_up", 103, 0xAF, 115),
    virtual_key("volume_down", 104, 0xAE, 114),
    plain("left_ctrl", 105, 0x1D),
    plain("left_shift", 106, 0x2A),
    plain("left_alt", 107, 0x38),
    extended("left_meta", 108, 0x5B, 125),
    extended("right_ctrl", 109, 0x1D, 97),
    plain("right_shift", 110, 0x36),
    extended("right_alt", 111, 0x38, 100),
    extended("right_meta", 112, 0x5C, 126),
    physical("brightness", 113),
    physical("win_lock", 114),
    physical("fn", 122),
    virtual_key("media_stop", 123, 0xB2, 166),
    virtual_key("play_pause", 124, 0xB3, 164),
    virtual_key("next_track", 125, 0xB0, 163),
    virtual_key("previous_track", 126, 0xB1, 165),
    physical("mr", 127),
    physical("profile", 128),
    physical("g1", 131),
    physical("g2", 132),
    physical("g3", 133),
    physical("g4", 134),
    physical("g5", 135),
    physical("g6", 136),
    physical("dial", 137),
    output_only("f13", 0x7C, 183),
    output_only("f14", 0x7D, 184),
    output_only("f15", 0x7E, 185),
    output_only("f16", 0x7F, 186),
    output_only("f17", 0x80, 187),
    output_only("f18", 0x81, 188),
    output_only("f19", 0x82, 189),
    output_only("f20", 0x83, 190),
    output_only("f21", 0x84, 191),
    output_only("f22", 0x85, 192),
    output_only("f23", 0x86, 193),
    output_only("f24", 0x87, 194),
];

pub fn all() -> &'static [KeyDef] {
    KEYS
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Key(&'static KeyDef);

impl Key {
    pub fn named(name: &str) -> Option<Self> {
        KEYS.iter().find(|def| def.name.eq_ignore_ascii_case(name)).map(Self)
    }

    pub fn from_index(index: u8) -> Option<Self> {
        KEYS.iter().find(|def| def.index == Some(index)).map(Self)
    }

    pub fn name(self) -> &'static str {
        self.0.name
    }

    pub fn index(self) -> Option<u8> {
        self.0.index
    }

    pub fn output(self) -> Option<Output> {
        self.0.output
    }

    pub fn is_on_k100(self) -> bool {
        self.0.is_on_k100()
    }

    pub fn is_modifier(self) -> bool {
        matches!(self.0.index, Some(105..=112))
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl TryFrom<String> for Key {
    type Error = String;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::named(&name).ok_or_else(|| format!("unknown key {name:?} (run `k100 keys` for the list)"))
    }
}

impl From<Key> for String {
    fn from(key: Key) -> Self {
        key.name().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn names_and_indices_are_unique() {
        let names: HashSet<&str> = all().iter().map(|def| def.name).collect();
        assert_eq!(names.len(), all().len());
        let indices: Vec<u8> = all().iter().filter_map(|def| def.index).collect();
        let unique: HashSet<u8> = indices.iter().copied().collect();
        assert_eq!(unique.len(), indices.len());
    }

    #[test]
    fn every_hid_usage_key_has_an_entry() {
        for index in 4..=101 {
            assert!(Key::from_index(index).is_some(), "missing key index {index}");
        }
    }

    #[test]
    fn lookups() {
        let a = Key::named("A").unwrap();
        assert_eq!((a.name(), a.index()), ("a", Some(4)));
        assert_eq!(a.output(), Some(Output { scan: 0x1E, vk: 0, evdev: 30 }));
        let up = Key::named("up").unwrap().output().unwrap();
        assert_eq!((up.scan, up.evdev, up.is_extended()), (0xE048, 103, true));
        let g1 = Key::named("g1").unwrap();
        assert_eq!((g1.index(), g1.output()), (Some(131), None));
        let f13 = Key::named("f13").unwrap();
        assert_eq!((f13.index(), f13.output()), (None, Some(Output { scan: 0, vk: 0x7C, evdev: 183 })));
        assert_eq!(Key::from_index(128).unwrap().name(), "profile");
        assert!(Key::named("left_shift").unwrap().is_modifier());
        assert!(a.is_on_k100() && g1.is_on_k100());
        assert!(!Key::named("mr").unwrap().is_on_k100());
        assert!(!Key::named("brightness").unwrap().is_on_k100());
        assert!(!f13.is_on_k100());
        assert!(!a.is_modifier());
    }

    #[test]
    fn serde_uses_names_and_explains_unknown_ones() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct Doc {
            key: Key,
        }
        let doc: Doc = toml::from_str("key = \"Caps_Lock\"").unwrap();
        assert_eq!(doc.key.index(), Some(57));
        assert_eq!(toml::to_string(&doc).unwrap().trim(), "key = \"caps_lock\"");
        let error = toml::from_str::<Doc>("key = \"capslock\"").unwrap_err().to_string();
        assert!(error.contains("k100 keys"), "{error}");
    }
}
