use crate::DecodeError;
use crate::wire::{Reader, Writer};

const TAIL_LEN: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilesList {
    pub roots: Vec<u16>,
}

impl ProfilesList {
    pub const HEADER: u16 = 0x0013;

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.u16(self.roots.len() as u16);
        for root in &self.roots {
            writer.u16(*root);
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let count = reader.u16()?;
        let roots = (0..count).map(|_| reader.u16()).collect::<Result<_, _>>()?;
        reader.finish()?;
        Ok(Self { roots })
    }
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    AssignmentMap = 0,
    PropertyMap = 1,
    Archive = 2,
    Lightings = 3,
    KeyMappings = 4,
    DialModes = 6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRoot {
    pub unknown: u32,
    pub sections: [u16; 12],
    pub name: String,
    pub tail: Vec<u16>,
}

impl ProfileRoot {
    pub const HEADER: u16 = 0xF11E;

    pub fn new(name: &str) -> Self {
        Self { unknown: 0, sections: [0; 12], name: name.to_string(), tail: vec![0; TAIL_LEN] }
    }

    pub fn section(&self, section: Section) -> u16 {
        self.sections[section as usize]
    }

    pub fn set(&mut self, section: Section, file: u16) {
        self.sections[section as usize] = file;
    }

    pub fn file_ids(&self) -> Vec<u16> {
        self.sections.iter().chain(&self.tail).copied().filter(|id| *id != 0).collect()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.u32(self.unknown);
        for section in self.sections {
            writer.u16(section);
        }
        writer.u16(self.name.len() as u16);
        writer.bytes(self.name.as_bytes());
        writer.u8(0);
        for value in &self.tail {
            writer.u16(*value);
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let unknown = reader.u32()?;
        let mut sections = [0; 12];
        for section in &mut sections {
            *section = reader.u16()?;
        }
        let name_len = reader.u16()? as usize;
        let name = String::from_utf8(reader.take(name_len)?.to_vec())
            .map_err(|_| DecodeError("profile name is not UTF-8".into()))?;
        match reader.u8()? {
            0 => {}
            other => return Err(DecodeError(format!("name terminator is {other:#04x}"))),
        }
        let tail = match reader.remaining() {
            0 => Vec::new(),
            left if left == TAIL_LEN * 2 => (0..TAIL_LEN).map(|_| reader.u16()).collect::<Result<_, _>>()?,
            left => {
                return Err(DecodeError(format!(
                    "{left} bytes follow the profile name, expected 0 or {}",
                    TAIL_LEN * 2
                )));
            }
        };
        Ok(Self { unknown, sections, name, tail })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::file;

    #[test]
    fn the_stored_profiles_list_round_trips() {
        let list = ProfilesList::decode(file(0x0F)).unwrap();
        assert_eq!(list.roots, [0x6D70]);
        assert_eq!(list.encode(), file(0x0F));
    }

    #[test]
    fn the_stored_root_round_trips_with_its_sections_and_name() {
        let root = ProfileRoot::decode(file(0x6D70)).unwrap();
        assert_eq!((root.name.as_str(), root.unknown), ("HW Profile 2", 0x30F));
        assert_eq!(root.section(Section::AssignmentMap), 0x6D69);
        assert_eq!(root.section(Section::Archive), 0x6D61);
        assert_eq!(root.section(Section::DialModes), 0x6D6E);
        assert_eq!(root.file_ids(), [0x6D69, 0x6D6D, 0x6D61, 0x6D6C, 0x6D6A, 0x6D6E]);
        assert_eq!(root.encode(), file(0x6D70));
    }

    #[test]
    fn a_new_root_is_the_long_form_with_only_the_sections_set() {
        let mut root = ProfileRoot::new("Hi");
        root.set(Section::Lightings, 0x6D72);
        let bytes = root.encode();
        assert_eq!(bytes.len(), 41 + 2);
        assert_eq!(&bytes[..6], [0x1E, 0xF1, 0, 0, 0, 0]);
        assert_eq!(&bytes[12..14], [0x72, 0x6D]);
        assert_eq!(&bytes[30..35], [2, 0, b'H', b'i', 0]);
        assert_eq!(ProfileRoot::decode(&bytes).unwrap(), root);
    }

    #[test]
    fn an_old_short_root_decodes_with_an_empty_tail() {
        let short = &file(0x6D70)[..33 + 12];
        let root = ProfileRoot::decode(short).unwrap();
        assert!(root.tail.is_empty());
        assert_eq!(root.encode(), short);
    }

    #[test]
    fn wrong_headers_and_truncated_files_are_decode_errors() {
        assert!(ProfilesList::decode(&[0x14, 0x00, 0x00, 0x00]).is_err());
        assert!(ProfilesList::decode(&[0x13, 0x00, 0x02, 0x00, 0x70, 0x6D]).is_err());
        assert!(ProfileRoot::decode(&file(0x6D70)[..20]).is_err());
    }

    #[test]
    fn trailing_bytes_and_a_bad_tail_are_decode_errors() {
        assert!(ProfilesList::decode(&[0x13, 0x00, 0x00, 0x00, 0x00]).is_err());
        let mut odd_tail = file(0x6D70).to_vec();
        odd_tail.push(0);
        assert!(ProfileRoot::decode(&odd_tail).is_err());
    }
}
