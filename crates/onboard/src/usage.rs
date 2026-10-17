use actions::Key;

const FIRST_KEYBOARD_INDEX: u8 = 4;
const LAST_KEYBOARD_INDEX: u8 = 101;
const FIRST_MODIFIER_INDEX: u8 = 105;
const LAST_MODIFIER_INDEX: u8 = 112;
const FIRST_MODIFIER_USAGE: u8 = 0xE0;
const FIRST_FUNCTION_USAGE: u8 = 0x68;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usage {
    Keyboard(u8),
    Consumer(u16),
}

pub fn hid_usage(key: Key) -> Option<Usage> {
    match key.index() {
        Some(index @ FIRST_KEYBOARD_INDEX..=LAST_KEYBOARD_INDEX) => Some(Usage::Keyboard(index)),
        Some(index @ FIRST_MODIFIER_INDEX..=LAST_MODIFIER_INDEX) => {
            Some(Usage::Keyboard(FIRST_MODIFIER_USAGE + (index - FIRST_MODIFIER_INDEX)))
        }
        _ => function_key_usage(key.name()).or_else(|| consumer_usage(key.name())),
    }
}

fn function_key_usage(name: &str) -> Option<Usage> {
    let number: u8 = name.strip_prefix('f')?.parse().ok()?;
    (13..=24).contains(&number).then(|| Usage::Keyboard(FIRST_FUNCTION_USAGE + (number - 13)))
}

fn consumer_usage(name: &str) -> Option<Usage> {
    let usage = match name {
        "mute" => 0xE2,
        "volume_up" => 0xE9,
        "volume_down" => 0xEA,
        "play_pause" => 0xCD,
        "next_track" => 0xB5,
        "previous_track" => 0xB6,
        "media_stop" => 0xB7,
        _ => return None,
    };
    Some(Usage::Consumer(usage))
}

#[cfg(test)]
mod tests {
    use actions::Key;

    use super::*;

    #[test]
    fn keys_map_to_hid_usages() {
        let usage = |name| hid_usage(Key::named(name).unwrap());
        for (name, expected) in [
            ("a", Usage::Keyboard(0x04)),
            ("escape", Usage::Keyboard(0x29)),
            ("backslash", Usage::Keyboard(0x31)),
            ("kp_0", Usage::Keyboard(0x62)),
            ("application", Usage::Keyboard(0x65)),
            ("left_ctrl", Usage::Keyboard(0xE0)),
            ("right_alt", Usage::Keyboard(0xE6)),
            ("right_meta", Usage::Keyboard(0xE7)),
            ("f13", Usage::Keyboard(0x68)),
            ("f24", Usage::Keyboard(0x73)),
            ("mute", Usage::Consumer(0xE2)),
            ("volume_up", Usage::Consumer(0xE9)),
            ("volume_down", Usage::Consumer(0xEA)),
            ("play_pause", Usage::Consumer(0xCD)),
            ("next_track", Usage::Consumer(0xB5)),
            ("previous_track", Usage::Consumer(0xB6)),
            ("media_stop", Usage::Consumer(0xB7)),
        ] {
            assert_eq!(usage(name), Some(expected), "{name}");
        }
        assert_eq!(usage("g1"), None);
    }
}
