use k100::Rgb;

use crate::DecodeError;
use crate::wire::{Reader, Writer};

pub const SPECIFIED_COLORS: u8 = 0x01;
pub const START_WITH_PROFILE: u8 = 0x08;

const TWO_BYTE_COUNTERS: u8 = 0x04;
const CHANNEL_LIST: u8 = 0x08;
const ALPHA_OPAQUE: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    SpiralRainbow,
    ColorWave,
    Visor,
    Rain,
    RippleType,
    GradientType,
    RainbowWave,
    Rainbow,
    ColorShift,
    ColorPulse,
    StaticColor,
    TypeWave,
}

impl EffectKind {
    const ALL: [Self; 12] = [
        Self::SpiralRainbow,
        Self::ColorWave,
        Self::Visor,
        Self::Rain,
        Self::RippleType,
        Self::GradientType,
        Self::RainbowWave,
        Self::Rainbow,
        Self::ColorShift,
        Self::ColorPulse,
        Self::StaticColor,
        Self::TypeWave,
    ];

    pub fn header(self) -> u16 {
        match self {
            Self::SpiralRainbow => 0xAB87,
            Self::ColorWave => 0x7BFF,
            Self::Visor => 0x90C0,
            Self::Rain => 0xA07E,
            Self::RippleType => 0x09A2,
            Self::GradientType => 0xB1F9,
            Self::RainbowWave => 0xB94C,
            Self::Rainbow => 0xA402,
            Self::ColorShift => 0xA5FA,
            Self::ColorPulse => 0xAD4F,
            Self::StaticColor => 0x207E,
            Self::TypeWave => 0x0028,
        }
    }

    pub fn from_header(header: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.header() == header)
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    None = 0,
    Slow = 3,
    Medium = 4,
    Fast = 5,
}

impl TryFrom<u8> for Speed {
    type Error = DecodeError;

    fn try_from(raw: u8) -> Result<Self, DecodeError> {
        match raw {
            0 => Ok(Self::None),
            3 => Ok(Self::Slow),
            4 => Ok(Self::Medium),
            5 => Ok(Self::Fast),
            other => Err(DecodeError(format!("unknown speed {other:#04x}"))),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    None = 0,
    Down = 1,
    Up = 2,
    Right = 4,
    Left = 5,
    Clockwise = 6,
    CounterClockwise = 7,
    ToCenter = 8,
    FromCenter = 9,
}

impl TryFrom<u8> for Direction {
    type Error = DecodeError;

    fn try_from(raw: u8) -> Result<Self, DecodeError> {
        match raw {
            0 => Ok(Self::None),
            1 => Ok(Self::Down),
            2 => Ok(Self::Up),
            4 => Ok(Self::Right),
            5 => Ok(Self::Left),
            6 => Ok(Self::Clockwise),
            7 => Ok(Self::CounterClockwise),
            8 => Ok(Self::ToCenter),
            9 => Ok(Self::FromCenter),
            other => Err(DecodeError(format!("unknown direction {other:#04x}"))),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectDuration {
    None = 0,
    Long = 3,
    Medium = 4,
    Short = 5,
}

impl TryFrom<u8> for EffectDuration {
    type Error = DecodeError;

    fn try_from(raw: u8) -> Result<Self, DecodeError> {
        match raw {
            0 => Ok(Self::None),
            3 => Ok(Self::Long),
            4 => Ok(Self::Medium),
            5 => Ok(Self::Short),
            other => Err(DecodeError(format!("unknown duration {other:#04x}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HwColor {
    pub alpha: u8,
    pub rgb: Rgb,
}

impl HwColor {
    pub fn opaque(rgb: Rgb) -> Self {
        Self { alpha: ALPHA_OPAQUE, rgb }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonLighting {
    pub kind: EffectKind,
    pub flags: u8,
    pub speed: Speed,
    pub direction: Direction,
    pub duration: EffectDuration,
    pub colors: Vec<HwColor>,
    pub leds: Vec<u8>,
}

impl CommonLighting {
    pub fn static_color(color: Rgb, leds: Vec<u8>) -> Self {
        Self {
            kind: EffectKind::StaticColor,
            flags: SPECIFIED_COLORS,
            speed: Speed::None,
            direction: Direction::None,
            duration: EffectDuration::None,
            colors: vec![HwColor::opaque(color)],
            leds,
        }
    }

    /// Colour and LED counts are one byte on the wire, so more than 255 of either panics.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(self.kind.header());
        writer.u8(self.flags);
        writer.u8(self.speed as u8);
        writer.u8(self.direction as u8);
        writer.u8(self.duration as u8);
        writer.u8(u8_count(self.colors.len(), "colours"));
        for color in &self.colors {
            writer.bytes(&[color.alpha, color.rgb.b, color.rgb.g, color.rgb.r]);
        }
        writer.u8(u8_count(self.leds.len(), "LEDs"));
        writer.bytes(&self.leds);
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let header = bytes.get(..2).map(|head| u16::from_le_bytes([head[0], head[1]]));
        let kind = header
            .and_then(EffectKind::from_header)
            .ok_or_else(|| DecodeError(format!("{header:x?} is not a common lighting effect header")))?;
        let mut reader = Reader::new(bytes, kind.header())?;
        let flags = reader.u8()?;
        if flags & (TWO_BYTE_COUNTERS | CHANNEL_LIST) != 0 {
            return Err(DecodeError(format!("lighting flags {flags:#04x} use two-byte counters or a channel list")));
        }
        let speed = Speed::try_from(reader.u8()?)?;
        let direction = Direction::try_from(reader.u8()?)?;
        let duration = EffectDuration::try_from(reader.u8()?)?;
        let color_count = reader.u8()?;
        let colors = (0..color_count)
            .map(|_| {
                let argb = reader.take(4)?;
                Ok(HwColor { alpha: argb[0], rgb: Rgb::new(argb[3], argb[2], argb[1]) })
            })
            .collect::<Result<_, DecodeError>>()?;
        let led_count = reader.u8()?;
        let leds = reader.take(led_count as usize)?.to_vec();
        reader.finish()?;
        Ok(Self { kind, flags, speed, direction, duration, colors, leds })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightingEntry {
    pub triggers: u8,
    pub reserved: u8,
    pub file: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightingsList {
    pub entries: Vec<LightingEntry>,
}

impl LightingsList {
    pub const HEADER: u16 = 0x6C69;

    /// The entry count is a u16 on the wire, so more than 65535 entries panics.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.u16(u16::try_from(self.entries.len()).expect("a lightings list holds at most 65535 entries"));
        for entry in &self.entries {
            writer.u8(entry.triggers);
            writer.u8(entry.reserved);
            writer.u16(entry.file);
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let count = reader.u16()?;
        let entries = (0..count)
            .map(|_| Ok(LightingEntry { triggers: reader.u8()?, reserved: reader.u8()?, file: reader.u16()? }))
            .collect::<Result<_, DecodeError>>()?;
        reader.finish()?;
        Ok(Self { entries })
    }
}

fn u8_count(len: usize, what: &str) -> u8 {
    u8::try_from(len).unwrap_or_else(|_| panic!("a lighting effect holds at most 255 {what}, got {len}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::file;

    #[test]
    fn the_stored_static_color_effect_round_trips() {
        let effect = CommonLighting::decode(file(0x6D6B)).unwrap();
        assert_eq!(effect.kind, EffectKind::StaticColor);
        assert_eq!(
            (effect.flags, effect.speed, effect.direction, effect.duration),
            (SPECIFIED_COLORS, Speed::None, Direction::None, EffectDuration::None)
        );
        assert_eq!(effect.colors, [HwColor::opaque(Rgb::new(255, 0, 0))]);
        assert_eq!((effect.leds.len(), effect.leds[0], effect.leds[115]), (116, 0x29, 0xBF));
        assert_eq!(effect.encode(), file(0x6D6B));
    }

    #[test]
    fn the_stored_lightings_list_round_trips() {
        let list = LightingsList::decode(file(0x6D6C)).unwrap();
        assert_eq!(list.entries, [LightingEntry { triggers: START_WITH_PROFILE, reserved: 0, file: 0x6D6B }]);
        assert_eq!(list.encode(), file(0x6D6C));
    }

    #[test]
    fn color_wave_encodes_as_in_the_format_doc() {
        let wave = CommonLighting {
            kind: EffectKind::ColorWave,
            flags: SPECIFIED_COLORS,
            speed: Speed::Medium,
            direction: Direction::Right,
            duration: EffectDuration::None,
            colors: vec![HwColor::opaque(Rgb::new(255, 0, 0)), HwColor::opaque(Rgb::new(0, 0, 255))],
            leds: vec![4, 5, 6, 7],
        };
        let expected = [
            0xFF, 0x7B, 0x01, 0x04, 0x04, 0x00, 0x02, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x04, 0x04, 0x05,
            0x06, 0x07,
        ];
        assert_eq!(wave.encode(), expected);
        assert_eq!(CommonLighting::decode(&expected).unwrap(), wave);
    }

    #[test]
    fn every_common_effect_header_is_known_and_the_generic_ones_are_not() {
        for kind in [EffectKind::SpiralRainbow, EffectKind::RippleType, EffectKind::StaticColor, EffectKind::TypeWave] {
            assert_eq!(EffectKind::from_header(kind.header()), Some(kind));
        }
        assert_eq!(EffectKind::GradientType.header(), 0xB1F9);
        for generic in [0x0087, 0xEBF4, 0x0AAE, 0x1FEA] {
            assert_eq!(EffectKind::from_header(generic), None);
        }
    }

    #[test]
    fn channel_lists_and_unknown_speeds_are_rejected() {
        let mut channels = file(0x6D6B).to_vec();
        channels[2] = 0x09;
        assert!(CommonLighting::decode(&channels).is_err());
        let mut speed = file(0x6D6B).to_vec();
        speed[3] = 0x02;
        assert!(CommonLighting::decode(&speed).is_err());
    }

    #[test]
    fn unknown_directions_durations_headers_and_trailing_bytes_are_rejected() {
        let mut direction = file(0x6D6B).to_vec();
        direction[4] = 0x03;
        assert!(CommonLighting::decode(&direction).is_err());
        let mut duration = file(0x6D6B).to_vec();
        duration[5] = 0x01;
        assert!(CommonLighting::decode(&duration).is_err());
        let mut two_byte = file(0x6D6B).to_vec();
        two_byte[2] = 0x05;
        assert!(CommonLighting::decode(&two_byte).is_err());
        let mut trailing = file(0x6D6B).to_vec();
        trailing.push(0);
        assert!(CommonLighting::decode(&trailing).is_err());
        assert!(CommonLighting::decode(&file(0x6D6B)[..40]).is_err());
        assert!(CommonLighting::decode(&[0x87, 0x00, 0x01]).is_err());
        assert!(CommonLighting::decode(&[0x7E]).is_err());
    }

    #[test]
    fn the_static_color_constructor_writes_one_opaque_colour() {
        let effect = CommonLighting::static_color(Rgb::new(1, 2, 3), vec![9, 8]);
        assert_eq!(effect.encode(), [0x7E, 0x20, 0x01, 0, 0, 0, 1, 0xFF, 3, 2, 1, 2, 9, 8]);
    }

    #[test]
    fn a_random_colour_effect_without_colours_round_trips() {
        let rainbow = CommonLighting {
            kind: EffectKind::Rainbow,
            flags: 0,
            speed: Speed::Fast,
            direction: Direction::None,
            duration: EffectDuration::None,
            colors: Vec::new(),
            leds: vec![4],
        };
        assert_eq!(CommonLighting::decode(&rainbow.encode()).unwrap(), rainbow);
    }

    #[test]
    fn a_lightings_list_with_several_entries_and_an_empty_one_round_trip() {
        let list = LightingsList {
            entries: vec![
                LightingEntry { triggers: START_WITH_PROFILE, reserved: 0, file: 0x6D61 },
                LightingEntry { triggers: 0x01, reserved: 0, file: 0x6D62 },
            ],
        };
        assert_eq!(list.encode(), [0x69, 0x6C, 2, 0, 8, 0, 0x61, 0x6D, 1, 0, 0x62, 0x6D]);
        assert_eq!(LightingsList::decode(&list.encode()).unwrap(), list);
        let empty = LightingsList { entries: Vec::new() };
        assert_eq!(empty.encode(), [0x69, 0x6C, 0, 0]);
        assert_eq!(LightingsList::decode(&empty.encode()).unwrap(), empty);
        assert!(LightingsList::decode(&[0x69, 0x6C, 1, 0, 8, 0, 0x61]).is_err());
        assert!(LightingsList::decode(&[0x69, 0x6C, 0, 0, 0]).is_err());
    }

    #[test]
    #[should_panic(expected = "at most 255 LEDs")]
    fn more_than_255_leds_is_refused_instead_of_truncated() {
        CommonLighting::static_color(Rgb::new(0, 0, 0), vec![4; 256]).encode();
    }

    #[test]
    fn exactly_255_leds_encode_with_their_full_count() {
        let bytes = CommonLighting::static_color(Rgb::new(0, 0, 0), vec![4; 255]).encode();
        assert_eq!(bytes[11], 255);
        assert_eq!(bytes.len(), 12 + 255);
    }
}
