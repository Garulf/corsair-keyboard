pub const MAX_FRAME: usize = 4 * 1024 * 1024;

const LENGTH_PREFIX: usize = 4;

pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(LENGTH_PREFIX + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes is over the 4 MiB limit")]
    TooLong(u32),
}

#[derive(Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let Some(prefix) = self.buffer.first_chunk::<LENGTH_PREFIX>() else {
            return Ok(None);
        };
        let length = u32::from_be_bytes(*prefix);
        if length as usize > MAX_FRAME {
            return Err(FrameError::TooLong(length));
        }
        let end = LENGTH_PREFIX + length as usize;
        if self.buffer.len() < end {
            return Ok(None);
        }
        let payload = self.buffer[LENGTH_PREFIX..end].to_vec();
        self.buffer.drain(..end);
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_carry_a_big_endian_length() {
        assert_eq!(encode_frame(b"{}"), [0, 0, 0, 2, b'{', b'}']);
        assert_eq!(encode_frame(&[7; 300])[..4], [0, 0, 1, 44]);
    }

    #[test]
    fn frames_split_across_reads_are_reassembled() {
        let mut decoder = FrameDecoder::default();
        for byte in encode_frame(b"hello") {
            assert_eq!(decoder.next_frame(), Ok(None));
            decoder.push(&[byte]);
        }
        assert_eq!(decoder.next_frame(), Ok(Some(b"hello".to_vec())));
        assert_eq!(decoder.next_frame(), Ok(None));
    }

    #[test]
    fn two_frames_in_one_read_come_out_in_order_and_empty_ones_are_empty() {
        let mut decoder = FrameDecoder::default();
        decoder.push(&[encode_frame(b"a"), encode_frame(b""), encode_frame(b"bc")].concat());
        assert_eq!(decoder.next_frame(), Ok(Some(b"a".to_vec())));
        assert_eq!(decoder.next_frame(), Ok(Some(Vec::new())));
        assert_eq!(decoder.next_frame(), Ok(Some(b"bc".to_vec())));
    }

    #[test]
    fn a_length_over_the_limit_is_an_error() {
        let mut decoder = FrameDecoder::default();
        decoder.push(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(decoder.next_frame(), Err(FrameError::TooLong(u32::MAX)));
    }

    #[test]
    fn a_frame_at_the_limit_is_accepted_and_one_past_it_is_not() {
        let mut at_limit = FrameDecoder::default();
        at_limit.push(&(MAX_FRAME as u32).to_be_bytes());
        assert_eq!(at_limit.next_frame(), Ok(None));
        let mut past_limit = FrameDecoder::default();
        past_limit.push(&(MAX_FRAME as u32 + 1).to_be_bytes());
        assert_eq!(past_limit.next_frame(), Err(FrameError::TooLong(MAX_FRAME as u32 + 1)));
    }
}
