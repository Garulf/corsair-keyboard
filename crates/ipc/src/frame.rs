use std::fmt::Write;

use k100::{Frame, LED_COUNT, Rgb};

pub fn encode_frame(frame: &Frame) -> String {
    let mut text = String::with_capacity(LED_COUNT * 6);
    for color in frame {
        let _ = write!(text, "{:02x}{:02x}{:02x}", color.r, color.g, color.b);
    }
    text
}

pub fn decode_frame(text: &str) -> Option<Frame> {
    if text.len() != LED_COUNT * 6 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut frame = [Rgb::BLACK; LED_COUNT];
    let (chunks, _) = text.as_bytes().as_chunks::<6>();
    for (slot, chunk) in frame.iter_mut().zip(chunks) {
        let channel = |at: usize| u8::from_str_radix(std::str::from_utf8(&chunk[at..at + 2]).ok()?, 16).ok();
        *slot = Rgb::new(channel(0)?, channel(2)?, channel(4)?);
    }
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Frame {
        let mut frame = [Rgb::BLACK; LED_COUNT];
        frame[0] = Rgb::new(1, 2, 3);
        frame[26] = Rgb::new(255, 128, 0);
        frame[LED_COUNT - 1] = Rgb::new(0xAB, 0xCD, 0xEF);
        frame
    }

    #[test]
    fn frames_round_trip_through_hex() {
        let text = encode_frame(&sample());
        assert_eq!(text.len(), LED_COUNT * 6);
        assert!(text.starts_with("010203"));
        assert_eq!(decode_frame(&text), Some(sample()));
    }

    #[test]
    fn malformed_frames_are_rejected() {
        let good = encode_frame(&sample());
        assert_eq!(decode_frame(""), None);
        assert_eq!(decode_frame(&good[..good.len() - 2]), None);
        assert_eq!(decode_frame(&format!("{good}00")), None);
        assert_eq!(decode_frame(&good.replacen("010203", "zz0203", 1)), None);
        assert_eq!(decode_frame(&good.replacen("010203", "+10203", 1)), None);
        assert_eq!(decode_frame(&"\u{e9}".repeat(LED_COUNT * 3)), None);
    }
}
