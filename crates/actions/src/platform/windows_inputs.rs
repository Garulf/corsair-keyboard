use crate::config::MouseButton;
use crate::keys::Output;
use crate::sink::{Axis, SinkEvent};

pub const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const KEYEVENTF_UNICODE: u32 = 0x0004;
pub const KEYEVENTF_SCANCODE: u32 = 0x0008;
pub const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
pub const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
pub const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
pub const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
pub const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
pub const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
pub const MOUSEEVENTF_XDOWN: u32 = 0x0080;
pub const MOUSEEVENTF_XUP: u32 = 0x0100;
pub const MOUSEEVENTF_WHEEL: u32 = 0x0800;
pub const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
pub const XBUTTON1: i32 = 1;
pub const XBUTTON2: i32 = 2;
const WHEEL_DELTA: i32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinInput {
    Key { vk: u16, scan: u16, flags: u32 },
    Mouse { flags: u32, data: i32 },
}

pub fn win_inputs(event: &SinkEvent) -> Vec<WinInput> {
    match event {
        SinkEvent::Key { key, down } => key.output().map(|output| key_input(output, *down)).into_iter().collect(),
        SinkEvent::Mouse { button, down } => vec![mouse_button(*button, *down)],
        SinkEvent::Wheel { axis, clicks } => {
            let flags = match axis {
                Axis::Vertical => MOUSEEVENTF_WHEEL,
                Axis::Horizontal => MOUSEEVENTF_HWHEEL,
            };
            vec![WinInput::Mouse { flags, data: clicks * WHEEL_DELTA }]
        }
        SinkEvent::Text(text) => text
            .encode_utf16()
            .flat_map(|unit| {
                [
                    WinInput::Key { vk: 0, scan: unit, flags: KEYEVENTF_UNICODE },
                    WinInput::Key { vk: 0, scan: unit, flags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP },
                ]
            })
            .collect(),
    }
}

fn key_input(output: Output, down: bool) -> WinInput {
    let up = if down { 0 } else { KEYEVENTF_KEYUP };
    if output.scan == 0 {
        return WinInput::Key { vk: output.vk, scan: 0, flags: up };
    }
    let extended = if output.is_extended() { KEYEVENTF_EXTENDEDKEY } else { 0 };
    WinInput::Key { vk: 0, scan: output.scan & 0xFF, flags: KEYEVENTF_SCANCODE | extended | up }
}

fn mouse_button(button: MouseButton, down: bool) -> WinInput {
    let (pressed, released, data) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
        MouseButton::Back => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, XBUTTON1),
        MouseButton::Forward => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, XBUTTON2),
    };
    WinInput::Mouse { flags: if down { pressed } else { released }, data }
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

    #[test]
    fn keys_use_scan_codes_or_virtual_keys() {
        assert_eq!(win_inputs(&key("a", true)), [WinInput::Key { vk: 0, scan: 0x1E, flags: KEYEVENTF_SCANCODE }]);
        assert_eq!(
            win_inputs(&key("a", false)),
            [WinInput::Key { vk: 0, scan: 0x1E, flags: KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP }]
        );
        assert_eq!(
            win_inputs(&key("up", true)),
            [WinInput::Key { vk: 0, scan: 0x48, flags: KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY }]
        );
        assert_eq!(win_inputs(&key("play_pause", true)), [WinInput::Key { vk: 0xB3, scan: 0, flags: 0 }]);
        assert!(win_inputs(&key("g1", true)).is_empty());
    }

    #[test]
    fn text_is_sent_as_utf16_units() {
        let inputs = win_inputs(&SinkEvent::Text("\u{e9}\u{1F600}".into()));
        assert_eq!(inputs.len(), 6);
        assert_eq!(inputs[0], WinInput::Key { vk: 0, scan: 0xE9, flags: KEYEVENTF_UNICODE });
        assert_eq!(inputs[1], WinInput::Key { vk: 0, scan: 0xE9, flags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP });
        assert_eq!(inputs[2], WinInput::Key { vk: 0, scan: 0xD83D, flags: KEYEVENTF_UNICODE });
    }

    #[test]
    fn mouse_buttons_and_wheels() {
        let back = SinkEvent::Mouse { button: MouseButton::Back, down: true };
        assert_eq!(win_inputs(&back), [WinInput::Mouse { flags: MOUSEEVENTF_XDOWN, data: XBUTTON1 }]);
        let left_up = SinkEvent::Mouse { button: MouseButton::Left, down: false };
        assert_eq!(win_inputs(&left_up), [WinInput::Mouse { flags: MOUSEEVENTF_LEFTUP, data: 0 }]);
        let wheel = SinkEvent::Wheel { axis: Axis::Vertical, clicks: -2 };
        assert_eq!(win_inputs(&wheel), [WinInput::Mouse { flags: MOUSEEVENTF_WHEEL, data: -240 }]);
        let hwheel = SinkEvent::Wheel { axis: Axis::Horizontal, clicks: 1 };
        assert_eq!(win_inputs(&hwheel), [WinInput::Mouse { flags: MOUSEEVENTF_HWHEEL, data: 120 }]);
    }
}
