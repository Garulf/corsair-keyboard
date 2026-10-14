pub const LED_COUNT: usize = 193;

const FRAME_HEADER: [u8; 2] = [0x12, 0x00];
const INDICATION_HEADER: [u8; 2] = [0x2B, 0x00];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

pub type Frame = [Rgb; LED_COUNT];

pub mod led {
    pub const MACRO_RECORDING_INDICATOR: u8 = 0;
    pub const MUTE_INDICATOR: u8 = 1;
    pub const MUTE: u8 = 102;
    pub const BRIGHTNESS: u8 = 113;
    pub const WIN_LOCK: u8 = 114;
    pub const FN: u8 = 122;
    pub const MEDIA_STOP: u8 = 123;
    pub const MEDIA_PLAY: u8 = 124;
    pub const MEDIA_NEXT: u8 = 125;
    pub const MEDIA_PREVIOUS: u8 = 126;
    pub const MACRO_RECORD: u8 = 127;
    pub const PROFILE: u8 = 128;
    pub const G1: u8 = 131;
    pub const G6: u8 = 136;
    pub const DIAL_KEY: u8 = 137;
    pub const LIGHT_BAR_FIRST: u8 = 138;
    pub const LIGHT_BAR_LAST: u8 = 159;
    pub const LEFT_UNDERGLOW_FIRST: u8 = 160;
    pub const LEFT_UNDERGLOW_LAST: u8 = 170;
    pub const RIGHT_UNDERGLOW_FIRST: u8 = 171;
    pub const RIGHT_UNDERGLOW_LAST: u8 = 181;
    pub const DIAL_RING_FIRST: u8 = 182;
    pub const DIAL_RING_LAST: u8 = 189;
    pub const LOGO_FIRST: u8 = 190;
    pub const LOGO_LAST: u8 = 192;
}

pub fn encode_frame(frame: &Frame) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FRAME_HEADER.len() + 3 * LED_COUNT);
    bytes.extend_from_slice(&FRAME_HEADER);
    for color in frame {
        bytes.extend_from_slice(&[color.r, color.g, color.b]);
    }
    bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndicatorRecord {
    pub led: u8,
    pub color: Rgb,
    pub alpha: u8,
}

pub fn encode_indicators(records: &[IndicatorRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(INDICATION_HEADER.len() + 5 * records.len());
    bytes.extend_from_slice(&INDICATION_HEADER);
    for record in records {
        bytes.extend_from_slice(&[record.led, record.color.r, record.color.g, record.color.b, record.alpha]);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_is_header_plus_rgb_triplets() {
        let mut frame = [Rgb::BLACK; LED_COUNT];
        frame[led::LOGO_FIRST as usize] = Rgb::new(1, 2, 3);
        let bytes = encode_frame(&frame);
        assert_eq!(bytes.len(), 581);
        assert_eq!(&bytes[..2], &[0x12, 0x00]);
        let offset = 2 + 3 * led::LOGO_FIRST as usize;
        assert_eq!(&bytes[offset..offset + 3], &[1, 2, 3]);
    }

    #[test]
    fn indicators_are_five_byte_records() {
        let records = [
            IndicatorRecord {
                led: led::DIAL_RING_FIRST,
                color: Rgb::new(255, 0, 0),
                alpha: 0xFF,
            },
            IndicatorRecord {
                led: led::WIN_LOCK,
                color: Rgb::new(0, 0, 9),
                alpha: 0,
            },
        ];
        assert_eq!(
            encode_indicators(&records),
            vec![0x2B, 0x00, 182, 255, 0, 0, 0xFF, 114, 0, 0, 9, 0]
        );
    }
}
