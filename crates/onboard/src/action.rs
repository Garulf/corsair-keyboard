use crate::DecodeError;
use crate::wire::{Reader, Writer};

pub mod execution {
    pub const REMAP_WHILE_PRESSED: u16 = 0x61;
    pub const MACRO_ON_PRESS: u16 = 0x31;
    pub const MACRO_WHILE_PRESSED: u16 = 0x71;
    pub const MACRO_TOGGLE: u16 = 0x39;
    pub const TEXT: u16 = 0x21;
}

const REMAP_KEYBOARD: u8 = 1;
const REMAP_MOUSE: u8 = 2;
const REMAP_CONSUMER: u8 = 3;
const REMAP_INTERNAL: u8 = 4;
const REMAP_GAMEPAD: u8 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemapTarget {
    Keyboard(Vec<u8>),
    Mouse(u8),
    Consumer(u16),
    Internal(u16),
    Gamepad(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemapAction {
    pub hold: u16,
    pub reserved: [u8; 3],
    pub target: RemapTarget,
}

impl RemapAction {
    pub const HEADER: u16 = 0x076E;

    pub fn new(target: RemapTarget) -> Self {
        Self { hold: 0, reserved: [0; 3], target }
    }

    /// A keyboard remap lists at most 255 usages, more panics.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.u8(match self.target {
            RemapTarget::Keyboard(_) => REMAP_KEYBOARD,
            RemapTarget::Mouse(_) => REMAP_MOUSE,
            RemapTarget::Consumer(_) => REMAP_CONSUMER,
            RemapTarget::Internal(_) => REMAP_INTERNAL,
            RemapTarget::Gamepad(_) => REMAP_GAMEPAD,
        });
        writer.u16(self.hold);
        writer.bytes(&self.reserved);
        match &self.target {
            RemapTarget::Keyboard(usages) => {
                writer.u8(u8::try_from(usages.len()).expect("a keyboard remap lists at most 255 keys"));
                writer.bytes(usages);
            }
            RemapTarget::Mouse(button) => writer.u8(*button),
            RemapTarget::Consumer(value) | RemapTarget::Internal(value) | RemapTarget::Gamepad(value) => {
                writer.u16(*value)
            }
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let remap_type = reader.u8()?;
        let hold = reader.u16()?;
        let reserved = [reader.u8()?, reader.u8()?, reader.u8()?];
        let target = match remap_type {
            REMAP_KEYBOARD => {
                let count = reader.u8()? as usize;
                RemapTarget::Keyboard(reader.take(count)?.to_vec())
            }
            REMAP_MOUSE => RemapTarget::Mouse(reader.u8()?),
            REMAP_CONSUMER => RemapTarget::Consumer(reader.u16()?),
            REMAP_INTERNAL => RemapTarget::Internal(reader.u16()?),
            REMAP_GAMEPAD => RemapTarget::Gamepad(reader.u16()?),
            other => return Err(DecodeError(format!("unknown remap type {other:#04x}"))),
        };
        reader.finish()?;
        Ok(Self { hold, reserved, target })
    }
}

const WORD_LEN: usize = 8;
const MACRO_RESERVED_LEN: usize = 12;
pub(crate) const DELAY24_MAX: u32 = 0xFF_FFFF;
const TWELVE_BIT_MIN: i16 = -2048;
const TWELVE_BIT_MAX: i16 = 2047;
const TWELVE_BIT_UNSIGNED_MAX: u16 = 0xFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroEvent {
    KeyDown(u8),
    KeyUp(u8),
    MouseDown(u8),
    MouseUp(u8),
    Move16 { x: i16, y: i16 },
    MoveOther { x: i16, y: i16 },
    Delay16(u16),
    RandomDelay16 { min: u16, max: u16 },
    HorizontalScroll(i8),
    VerticalScroll(i8),
    Move12 { x: i16, y: i16 },
    Delay8(u8),
    RandomDelay12 { min: u16, max: u16 },
    Delay24(u32),
}

impl MacroEvent {
    pub fn delay(ms: u32) -> Self {
        match ms {
            0..=0xFF => Self::Delay8(ms as u8),
            0x100..=0xFFFF => Self::Delay16(ms as u16),
            _ => Self::Delay24(ms.min(DELAY24_MAX)),
        }
    }

    fn opcode(self) -> u8 {
        match self {
            Self::KeyDown(_) => 0x01,
            Self::KeyUp(_) => 0x02,
            Self::MouseDown(_) => 0x03,
            Self::MouseUp(_) => 0x04,
            Self::Move16 { .. } => 0x05,
            Self::MoveOther { .. } => 0x06,
            Self::Delay16(_) => 0x07,
            Self::RandomDelay16 { .. } => 0x08,
            Self::HorizontalScroll(_) => 0x09,
            Self::VerticalScroll(_) => 0x0A,
            Self::Move12 { .. } => 0x0B,
            Self::Delay8(_) => 0x0C,
            Self::RandomDelay12 { .. } => 0x0D,
            Self::Delay24(_) => 0x0E,
        }
    }

    fn payload_len(opcode: u8) -> Option<usize> {
        match opcode {
            0x01..=0x04 | 0x09 | 0x0A | 0x0C => Some(1),
            0x07 => Some(2),
            0x0B | 0x0D | 0x0E => Some(3),
            0x05 | 0x06 | 0x08 => Some(4),
            _ => None,
        }
    }

    fn encode(self) -> Vec<u8> {
        let mut bytes = vec![self.opcode()];
        match self {
            Self::KeyDown(value) | Self::KeyUp(value) | Self::MouseDown(value) | Self::MouseUp(value) => {
                bytes.push(value)
            }
            Self::Delay8(value) => bytes.push(value),
            Self::HorizontalScroll(value) | Self::VerticalScroll(value) => bytes.push(value as u8),
            Self::Move16 { x, y } | Self::MoveOther { x, y } => {
                bytes.extend(x.to_le_bytes());
                bytes.extend(y.to_le_bytes());
            }
            Self::Delay16(value) => bytes.extend(value.to_le_bytes()),
            Self::RandomDelay16 { min, max } => {
                bytes.extend(min.to_le_bytes());
                bytes.extend(max.to_le_bytes());
            }
            Self::Move12 { x, y } => {
                assert!(
                    (TWELVE_BIT_MIN..=TWELVE_BIT_MAX).contains(&x) && (TWELVE_BIT_MIN..=TWELVE_BIT_MAX).contains(&y),
                    "a 12-bit mouse move holds -2048..=2047, got ({x}, {y})"
                );
                bytes.extend(pack_twelve_bit(x as u16 & TWELVE_BIT_UNSIGNED_MAX, y as u16 & TWELVE_BIT_UNSIGNED_MAX));
            }
            Self::RandomDelay12 { min, max } => {
                assert!(
                    min <= TWELVE_BIT_UNSIGNED_MAX && max <= TWELVE_BIT_UNSIGNED_MAX,
                    "a 12-bit random delay holds at most 4095, got {min}..{max}"
                );
                bytes.extend(pack_twelve_bit(min, max));
            }
            Self::Delay24(value) => {
                assert!(value <= DELAY24_MAX, "a 24-bit delay holds at most {DELAY24_MAX}, got {value}");
                bytes.extend(&value.to_le_bytes()[..3]);
            }
        }
        bytes
    }

    fn decode(opcode: u8, payload: &[u8]) -> Self {
        let signed = |at: usize| i16::from_le_bytes([payload[at], payload[at + 1]]);
        let unsigned = |at: usize| u16::from_le_bytes([payload[at], payload[at + 1]]);
        match opcode {
            0x01 => Self::KeyDown(payload[0]),
            0x02 => Self::KeyUp(payload[0]),
            0x03 => Self::MouseDown(payload[0]),
            0x04 => Self::MouseUp(payload[0]),
            0x05 => Self::Move16 { x: signed(0), y: signed(2) },
            0x06 => Self::MoveOther { x: signed(0), y: signed(2) },
            0x07 => Self::Delay16(unsigned(0)),
            0x08 => Self::RandomDelay16 { min: unsigned(0), max: unsigned(2) },
            0x09 => Self::HorizontalScroll(payload[0] as i8),
            0x0A => Self::VerticalScroll(payload[0] as i8),
            0x0B => {
                let (x, y) = unpack_twelve_bit(payload);
                Self::Move12 { x: sign_extend_twelve_bit(x), y: sign_extend_twelve_bit(y) }
            }
            0x0C => Self::Delay8(payload[0]),
            0x0D => {
                let (min, max) = unpack_twelve_bit(payload);
                Self::RandomDelay12 { min, max }
            }
            _ => Self::Delay24(u32::from_le_bytes([payload[0], payload[1], payload[2], 0])),
        }
    }
}

fn pack_twelve_bit(first: u16, second: u16) -> [u8; 3] {
    [(first & 0xFF) as u8, ((first >> 8) | ((second & 0xF) << 4)) as u8, (second >> 4) as u8]
}

fn unpack_twelve_bit(payload: &[u8]) -> (u16, u16) {
    let first = payload[0] as u16 | ((payload[1] as u16 & 0xF) << 8);
    let second = (payload[1] as u16 >> 4) | ((payload[2] as u16) << 4);
    (first, second)
}

fn sign_extend_twelve_bit(value: u16) -> i16 {
    ((value << 4) as i16) >> 4
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroAction {
    pub events: Vec<MacroEvent>,
}

impl MacroAction {
    pub const HEADER: u16 = 0x30AC;

    /// The word count is a u16 on the wire, so more than 65535 words panics.
    pub fn encode(&self) -> Vec<u8> {
        let words = self.pack_words();
        let mut writer = Writer::new(Self::HEADER);
        writer.u16(u16::try_from(words.len()).expect("a macro holds at most 65535 words"));
        writer.bytes(&[0; MACRO_RESERVED_LEN]);
        for word in &words {
            writer.bytes(word);
        }
        writer.finish()
    }

    fn pack_words(&self) -> Vec<[u8; WORD_LEN]> {
        let mut words = vec![[0; WORD_LEN]];
        let mut used = 0;
        for event in &self.events {
            let bytes = event.encode();
            if used + bytes.len() > WORD_LEN {
                words.push([0; WORD_LEN]);
                used = 0;
            }
            let word = words.last_mut().expect("words always holds the current word");
            word[used..used + bytes.len()].copy_from_slice(&bytes);
            used += bytes.len();
        }
        words
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let word_count = reader.u16()?;
        reader.take(MACRO_RESERVED_LEN)?;
        let mut events = Vec::new();
        for _ in 0..word_count {
            Self::decode_word(reader.take(WORD_LEN)?, &mut events)?;
        }
        reader.finish()?;
        Ok(Self { events })
    }

    fn decode_word(word: &[u8], events: &mut Vec<MacroEvent>) -> Result<(), DecodeError> {
        let mut at = 0;
        while at < word.len() && word[at] != 0 {
            let opcode = word[at];
            let payload_len = MacroEvent::payload_len(opcode)
                .ok_or_else(|| DecodeError(format!("unknown macro opcode {opcode:#04x}")))?;
            let payload = word
                .get(at + 1..at + 1 + payload_len)
                .ok_or_else(|| DecodeError(format!("macro opcode {opcode:#04x} runs past its word")))?;
            events.push(MacroEvent::decode(opcode, payload));
            at += 1 + payload_len;
        }
        Ok(())
    }
}

const TEXT_RESERVED_LEN: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAction {
    pub options: u32,
    pub unknown: u16,
    pub text: Vec<u8>,
}

impl TextAction {
    pub const HEADER: u16 = 0x03AC;

    pub fn new(text: Vec<u8>) -> Self {
        Self { options: 0, unknown: 0, text }
    }

    /// The text length is a u16 on the wire, so more than 65535 bytes panics.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.u32(self.options);
        writer.u16(u16::try_from(self.text.len()).expect("a text action holds at most 65535 bytes"));
        writer.u16(self.unknown);
        writer.bytes(&[0; TEXT_RESERVED_LEN]);
        writer.bytes(&self.text);
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let options = reader.u32()?;
        let length = reader.u16()? as usize;
        let unknown = reader.u16()?;
        reader.take(TEXT_RESERVED_LEN)?;
        let text = reader.take(length)?.to_vec();
        reader.finish()?;
        Ok(Self { options, unknown, text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::file;

    #[test]
    fn the_stored_remap_actions_round_trip() {
        for id in 0x6D62..=0x6D68 {
            assert_eq!(RemapAction::decode(file(id)).unwrap().encode(), file(id), "{id:#x}");
        }
        let caps = RemapAction::decode(file(0x6D62)).unwrap();
        assert_eq!(caps, RemapAction::new(RemapTarget::Keyboard(vec![0xE6, 0xE5, 0x31])));
    }

    #[test]
    fn consumer_and_mouse_remaps_encode_as_in_the_format_doc() {
        assert_eq!(
            RemapAction::new(RemapTarget::Consumer(0xE9)).encode(),
            [0x6E, 0x07, 0x03, 0, 0, 0, 0, 0, 0xE9, 0x00]
        );
        assert_eq!(RemapAction::new(RemapTarget::Mouse(3)).encode(), [0x6E, 0x07, 0x02, 0, 0, 0, 0, 0, 0x03]);
    }

    #[test]
    fn internal_and_gamepad_remaps_round_trip() {
        for target in [RemapTarget::Internal(8), RemapTarget::Gamepad(0x10), RemapTarget::Mouse(5)] {
            let action = RemapAction { hold: 1, reserved: [0, 0, 0], target };
            assert_eq!(RemapAction::decode(&action.encode()).unwrap(), action);
        }
    }

    #[test]
    fn malformed_remaps_are_decode_errors() {
        assert!(RemapAction::decode(&[0x6E, 0x07, 0x09, 0, 0, 0, 0, 0, 1]).is_err());
        assert!(RemapAction::decode(&[0x6E, 0x07, 0x03, 0, 0, 0, 0, 0, 0xE9]).is_err());
        assert!(RemapAction::decode(&[0x6E, 0x07, 0x02, 0, 0, 0, 0, 0, 3, 0]).is_err());
        assert!(RemapAction::decode(&[0x6E, 0x07, 0x01, 0, 0, 0, 0, 0, 2, 0xE0]).is_err());
    }

    #[test]
    fn the_macro_example_packs_events_without_splitting_them() {
        use MacroEvent::*;
        let action = MacroAction { events: vec![KeyDown(4), Delay8(50), KeyUp(4), KeyDown(5), KeyUp(5)] };
        let mut expected = vec![0xAC, 0x30, 0x02, 0x00];
        expected.extend([0; 12]);
        expected.extend([0x01, 0x04, 0x0C, 0x32, 0x02, 0x04, 0x01, 0x05, 0x02, 0x05, 0, 0, 0, 0, 0, 0]);
        assert_eq!(action.encode(), expected);
        assert_eq!(MacroAction::decode(&expected).unwrap(), action);
    }

    #[test]
    fn every_macro_event_round_trips() {
        use MacroEvent::*;
        let events = vec![
            KeyDown(0xE0),
            KeyUp(0xE0),
            MouseDown(1),
            MouseUp(5),
            Move16 { x: -3000, y: 4000 },
            MoveOther { x: 7, y: -7 },
            Delay16(300),
            RandomDelay16 { min: 5000, max: 9000 },
            HorizontalScroll(-2),
            VerticalScroll(3),
            Move12 { x: -5, y: 2 },
            Move12 { x: 5, y: -3 },
            Move12 { x: -2048, y: 2047 },
            Delay8(0),
            RandomDelay12 { min: 20, max: 4095 },
            Delay24(70_000),
        ];
        let action = MacroAction { events };
        assert_eq!(MacroAction::decode(&action.encode()).unwrap(), action);
    }

    #[test]
    fn twelve_bit_moves_mask_their_signed_halves() {
        let action = MacroAction { events: vec![MacroEvent::Move12 { x: -5, y: 2 }] };
        assert_eq!(&action.encode()[16..20], [0x0B, 0xFB, 0x2F, 0x00]);
    }

    #[test]
    fn random_delays_pack_two_twelve_bit_values() {
        let action = MacroAction { events: vec![MacroEvent::RandomDelay12 { min: 20, max: 80 }] };
        assert_eq!(&action.encode()[16..20], [0x0D, 0x14, 0x00, 0x05]);
    }

    fn first_word(event: MacroEvent) -> [u8; WORD_LEN] {
        MacroAction { events: vec![event] }.encode()[16..].try_into().unwrap()
    }

    #[test]
    fn sixteen_and_twenty_four_bit_delays_encode_little_endian() {
        assert_eq!(first_word(MacroEvent::Delay16(300)), [0x07, 0x2C, 0x01, 0, 0, 0, 0, 0]);
        assert_eq!(first_word(MacroEvent::Delay16(0xFFFF)), [0x07, 0xFF, 0xFF, 0, 0, 0, 0, 0]);
        assert_eq!(
            first_word(MacroEvent::RandomDelay16 { min: 5000, max: 9000 }),
            [0x08, 0x88, 0x13, 0x28, 0x23, 0, 0, 0]
        );
        assert_eq!(first_word(MacroEvent::Delay24(70_000)), [0x0E, 0x70, 0x11, 0x01, 0, 0, 0, 0]);
    }

    #[test]
    fn scroll_events_encode_their_signed_step() {
        assert_eq!(first_word(MacroEvent::VerticalScroll(-2)), [0x0A, 0xFE, 0, 0, 0, 0, 0, 0]);
        assert_eq!(first_word(MacroEvent::VerticalScroll(3)), [0x0A, 0x03, 0, 0, 0, 0, 0, 0]);
        assert_eq!(first_word(MacroEvent::HorizontalScroll(-1)), [0x09, 0xFF, 0, 0, 0, 0, 0, 0]);
        assert_eq!(first_word(MacroEvent::HorizontalScroll(5)), [0x09, 0x05, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn delays_use_the_smallest_opcode() {
        assert_eq!(MacroEvent::delay(50), MacroEvent::Delay8(50));
        assert_eq!(MacroEvent::delay(300), MacroEvent::Delay16(300));
        assert_eq!(MacroEvent::delay(70_000), MacroEvent::Delay24(70_000));
        assert_eq!(MacroEvent::delay(u32::MAX), MacroEvent::Delay24(0xFF_FFFF));
    }

    #[test]
    fn an_empty_macro_is_one_zero_word() {
        let encoded = MacroAction { events: Vec::new() }.encode();
        assert_eq!(encoded.len(), 4 + 12 + 8);
        assert_eq!(&encoded[2..4], [1, 0]);
        assert_eq!(MacroAction::decode(&encoded).unwrap().events, []);
    }

    #[test]
    fn malformed_macros_are_decode_errors() {
        let mut bad_opcode = vec![0xAC, 0x30, 0x01, 0x00];
        bad_opcode.extend([0; 12]);
        bad_opcode.extend([0x7F, 0, 0, 0, 0, 0, 0, 0]);
        assert!(MacroAction::decode(&bad_opcode).is_err());

        let mut straddling = vec![0xAC, 0x30, 0x01, 0x00];
        straddling.extend([0; 12]);
        straddling.extend([0x01, 0x04, 0x01, 0x04, 0x01, 0x04, 0x05, 0x01]);
        assert!(MacroAction::decode(&straddling).is_err());

        let mut short = vec![0xAC, 0x30, 0x02, 0x00];
        short.extend([0; 12]);
        short.extend([0; 8]);
        assert!(MacroAction::decode(&short).is_err());
    }

    #[test]
    fn text_actions_encode_as_in_the_format_doc() {
        let text = TextAction::new(b"hi".to_vec());
        let expected = [0xAC, 0x03, 0, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x68, 0x69];
        assert_eq!(text.encode(), expected);
        assert_eq!(TextAction::decode(&expected).unwrap(), text);
    }

    #[test]
    fn text_options_and_the_unknown_word_survive_a_round_trip() {
        let text = TextAction { options: 0x0001_0A00, unknown: 7, text: b"x".to_vec() };
        assert_eq!(TextAction::decode(&text.encode()).unwrap(), text);
    }

    #[test]
    fn a_text_shorter_than_its_length_is_a_decode_error() {
        let mut bytes = TextAction::new(b"hello".to_vec()).encode();
        bytes.pop();
        assert!(TextAction::decode(&bytes).is_err());
    }
}
