use crate::DecodeError;

pub(crate) struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub(crate) fn new(header: u16) -> Self {
        let mut writer = Self { bytes: Vec::new() };
        writer.u16(header);
        writer
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8], header: u16) -> Result<Self, DecodeError> {
        let mut reader = Self { bytes };
        let found = reader.u16()?;
        if found != header {
            return Err(DecodeError(format!("expected header {header:#06x}, found {found:#06x}")));
        }
        Ok(reader)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if self.bytes.len() < count {
            return Err(DecodeError(format!("needed {count} more bytes, {} left", self.bytes.len())));
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        match self.remaining() {
            0 => Ok(()),
            left => Err(DecodeError(format!("{left} unexpected trailing bytes"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_little_endian_after_the_header() {
        let mut writer = Writer::new(0xF11E);
        writer.u8(1);
        writer.u16(0x0302);
        writer.u32(0x07060504);
        writer.bytes(&[8, 9]);
        let bytes = writer.finish();
        assert_eq!(bytes, [0x1E, 0xF1, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        let mut reader = Reader::new(&bytes, 0xF11E).unwrap();
        assert_eq!((reader.u8().unwrap(), reader.u16().unwrap(), reader.u32().unwrap()), (1, 0x0302, 0x07060504));
        assert_eq!(reader.remaining(), 2);
        assert_eq!(reader.take(2).unwrap(), [8, 9]);
        assert!(reader.is_empty());
    }

    #[test]
    fn a_wrong_header_or_short_read_is_a_decode_error() {
        assert!(Reader::new(&[0x01, 0x00], 0x02).is_err());
        assert!(Reader::new(&[0x01], 0x01).is_err());
        let mut reader = Reader::new(&[0x01, 0x00, 0xFF], 0x01).unwrap();
        assert!(reader.u16().is_err());
        assert!(reader.finish().is_err());
    }
}
