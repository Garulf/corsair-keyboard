use crate::config::MouseButton;
use crate::keys::Key;
use crate::sink::{Axis, SinkError, SinkEvent};

pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_HWHEEL: u16 = 0x06;
pub const REL_WHEEL: u16 = 0x08;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_SIDE: u16 = 0x113;
pub const BTN_EXTRA: u16 = 0x114;

const SHIFTED: &[(char, &str)] = &[
    ('!', "1"),
    ('@', "2"),
    ('#', "3"),
    ('$', "4"),
    ('%', "5"),
    ('^', "6"),
    ('&', "7"),
    ('*', "8"),
    ('(', "9"),
    (')', "0"),
    ('_', "minus"),
    ('+', "equal"),
    ('{', "left_bracket"),
    ('}', "right_bracket"),
    ('|', "backslash"),
    (':', "semicolon"),
    ('"', "apostrophe"),
    ('~', "grave"),
    ('<', "comma"),
    ('>', "period"),
    ('?', "slash"),
];

const UNSHIFTED: &[(char, &str)] = &[
    (' ', "space"),
    ('\n', "enter"),
    ('\t', "tab"),
    ('-', "minus"),
    ('=', "equal"),
    ('[', "left_bracket"),
    (']', "right_bracket"),
    ('\\', "backslash"),
    (';', "semicolon"),
    ('\'', "apostrophe"),
    ('`', "grave"),
    (',', "comma"),
    ('.', "period"),
    ('/', "slash"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

pub fn linux_events(event: &SinkEvent) -> Result<Vec<RawEvent>, SinkError> {
    Ok(match event {
        SinkEvent::Key { key, down } => key_events(*key, *down).into_iter().collect(),
        SinkEvent::Mouse { button, down } => {
            vec![RawEvent { kind: EV_KEY, code: button_code(*button), value: i32::from(*down) }]
        }
        SinkEvent::Wheel { axis, clicks } => {
            let code = match axis {
                Axis::Vertical => REL_WHEEL,
                Axis::Horizontal => REL_HWHEEL,
            };
            vec![RawEvent { kind: EV_REL, code, value: *clicks }]
        }
        SinkEvent::Text(text) => text_events(text)?,
    })
}

fn key_events(key: Key, down: bool) -> Option<RawEvent> {
    key.output().map(|output| RawEvent { kind: EV_KEY, code: output.evdev, value: i32::from(down) })
}

fn button_code(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
        MouseButton::Back => BTN_SIDE,
        MouseButton::Forward => BTN_EXTRA,
    }
}

fn text_events(text: &str) -> Result<Vec<RawEvent>, SinkError> {
    let shift = Key::named("left_shift").expect("left_shift is in the key table");
    let mut events = Vec::new();
    for character in text.chars() {
        let (key, shifted) = us_key(character).ok_or_else(|| {
            SinkError(format!("cannot type {character:?}: only US-layout ASCII text is supported on Linux"))
        })?;
        let mut push = |key: Key, down: bool| events.extend(key_events(key, down));
        if shifted {
            push(shift, true);
        }
        push(key, true);
        push(key, false);
        if shifted {
            push(shift, false);
        }
    }
    Ok(events)
}

fn us_key(character: char) -> Option<(Key, bool)> {
    if character.is_ascii_lowercase() || character.is_ascii_digit() {
        return Key::named(&character.to_string()).map(|key| (key, false));
    }
    if character.is_ascii_uppercase() {
        return Key::named(&character.to_ascii_lowercase().to_string()).map(|key| (key, true));
    }
    let lookup = |table: &[(char, &str)]| table.iter().find(|(c, _)| *c == character).and_then(|(_, n)| Key::named(n));
    lookup(UNSHIFTED).map(|key| (key, false)).or_else(|| lookup(SHIFTED).map(|key| (key, true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MouseButton;
    use crate::keys::Key;
    use crate::sink::{Axis, SinkEvent};

    fn key(name: &str, down: bool) -> SinkEvent {
        SinkEvent::key(Key::named(name).unwrap(), down)
    }

    fn press(code: u16, value: i32) -> RawEvent {
        RawEvent { kind: EV_KEY, code, value }
    }

    #[test]
    fn keys_buttons_and_wheels() {
        assert_eq!(linux_events(&key("a", true)).unwrap(), [press(30, 1)]);
        assert_eq!(linux_events(&key("right_ctrl", false)).unwrap(), [press(97, 0)]);
        assert_eq!(linux_events(&key("f13", true)).unwrap(), [press(183, 1)]);
        assert!(linux_events(&key("g1", true)).unwrap().is_empty());
        let forward = SinkEvent::Mouse { button: MouseButton::Forward, down: true };
        assert_eq!(linux_events(&forward).unwrap(), [press(BTN_EXTRA, 1)]);
        let hwheel = SinkEvent::Wheel { axis: Axis::Horizontal, clicks: 1 };
        assert_eq!(linux_events(&hwheel).unwrap(), [RawEvent { kind: EV_REL, code: REL_HWHEEL, value: 1 }]);
    }

    #[test]
    fn text_types_us_ascii_with_shift() {
        let events = linux_events(&SinkEvent::Text("Hi!".into())).unwrap();
        assert_eq!(
            events,
            [
                press(42, 1),
                press(35, 1),
                press(35, 0),
                press(42, 0),
                press(23, 1),
                press(23, 0),
                press(42, 1),
                press(2, 1),
                press(2, 0),
                press(42, 0),
            ]
        );
    }

    #[test]
    fn text_outside_us_ascii_fails_without_typing_anything() {
        let error = linux_events(&SinkEvent::Text("ok \u{e9}".into())).unwrap_err();
        assert!(error.0.contains("US"), "{error}");
    }
}
