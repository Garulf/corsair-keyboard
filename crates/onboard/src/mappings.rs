use k100::Rgb;

use crate::DecodeError;
use crate::wire::{Reader, Writer};

const ALPHA_OPAQUE: u8 = 0xFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredKeyMappings {
    pub bytes: Vec<u8>,
}

impl StoredKeyMappings {
    pub const HEADER: u16 = 0x0BAF;

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.bytes(&self.bytes);
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let rest = reader.take(reader.remaining())?;
        Ok(Self { bytes: rest.to_vec() })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssignmentEntry {
    pub key: u8,
    pub modifier: u16,
    pub action: u16,
    pub execution: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentMap {
    pub entries: Vec<AssignmentEntry>,
}

impl AssignmentMap {
    pub const HEADER: u16 = 0xAC3D;

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.u16(0);
        for entry in &self.entries {
            writer.u8(entry.key);
            writer.u8(1);
            writer.u16(entry.modifier);
            writer.u16(entry.action);
            writer.u16(entry.execution);
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        reader.u16()?;
        let mut entries = Vec::new();
        while !reader.is_empty() {
            let key = reader.u8()?;
            let items = reader.u8()?;
            let modifier = reader.u16()?;
            for _ in 0..items {
                let action = reader.u16()?;
                let execution = reader.u16()?;
                entries.push(AssignmentEntry { key, modifier, action, execution });
            }
        }
        Ok(Self { entries })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialAction {
    pub event: u8,
    pub unknown: u8,
    pub options: u16,
    pub action: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialModeEntry {
    pub kind: u8,
    pub color: [u8; 4],
    pub actions: Vec<DialAction>,
}

impl DialModeEntry {
    pub fn new(kind: u8, color: Rgb) -> Self {
        Self { kind, color: [ALPHA_OPAQUE, color.b, color.g, color.r], actions: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialModes {
    pub modes: Vec<DialModeEntry>,
}

impl DialModes {
    pub const HEADER: u16 = 0x0016;

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(Self::HEADER);
        writer.u16(self.modes.len() as u16);
        for mode in &self.modes {
            writer.u8(mode.kind);
            writer.bytes(&mode.color);
            writer.u8(mode.actions.len() as u8);
            for action in &mode.actions {
                writer.u8(action.event);
                writer.u8(action.unknown);
                writer.u16(action.options);
                writer.u16(action.action);
            }
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes, Self::HEADER)?;
        let count = reader.u16()?;
        let modes = (0..count).map(|_| Self::decode_mode(&mut reader)).collect::<Result<_, _>>()?;
        reader.finish()?;
        Ok(Self { modes })
    }

    fn decode_mode(reader: &mut Reader) -> Result<DialModeEntry, DecodeError> {
        let kind = reader.u8()?;
        let color = reader.take(4)?.try_into().expect("take(4) returns four bytes");
        let action_count = reader.u8()?;
        let actions = (0..action_count)
            .map(|_| {
                Ok(DialAction {
                    event: reader.u8()?,
                    unknown: reader.u8()?,
                    options: reader.u16()?,
                    action: reader.u16()?,
                })
            })
            .collect::<Result<_, DecodeError>>()?;
        Ok(DialModeEntry { kind, color, actions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KEY_COUNT;
    use crate::fixtures::file;

    #[test]
    fn the_stored_assignment_map_round_trips() {
        let map = AssignmentMap::decode(file(0x6D69)).unwrap();
        assert_eq!(map.entries.len(), 7);
        assert_eq!(map.entries[0], AssignmentEntry { key: 57, modifier: 0, action: 0x6D62, execution: 0x61 });
        assert_eq!(map.entries[2].key, 131);
        assert_eq!(map.encode(), file(0x6D69));
    }

    #[test]
    fn entries_with_several_items_are_flattened() {
        let bytes = [0x3D, 0xAC, 0, 0, 0x83, 0x02, 0, 0, 0x62, 0x6D, 0x61, 0x00, 0x63, 0x6D, 0x31, 0x00];
        let map = AssignmentMap::decode(&bytes).unwrap();
        assert_eq!(
            map.entries,
            [
                AssignmentEntry { key: 0x83, modifier: 0, action: 0x6D62, execution: 0x61 },
                AssignmentEntry { key: 0x83, modifier: 0, action: 0x6D63, execution: 0x31 },
            ]
        );
    }

    #[test]
    fn a_truncated_assignment_entry_is_a_decode_error() {
        assert!(AssignmentMap::decode(&[0x3D, 0xAC, 0, 0, 0x83, 0x01, 0, 0, 0x62]).is_err());
    }

    #[test]
    fn the_stored_key_mappings_and_dial_modes_round_trip() {
        let mappings = StoredKeyMappings::decode(file(0x6D6A)).unwrap();
        assert_eq!(mappings.bytes.len(), KEY_COUNT);
        assert_eq!((mappings.bytes[57], mappings.bytes[131], mappings.bytes[4]), (0x00, 0x00, 0x01));
        assert_eq!(mappings.encode(), file(0x6D6A));
        let dial = DialModes::decode(file(0x6D6E)).unwrap();
        assert_eq!(dial.modes, [DialModeEntry::new(0x01, Rgb::new(0x00, 0xBF, 0xFF))]);
        assert_eq!(dial.encode(), file(0x6D6E));
    }

    #[test]
    fn dial_mode_actions_round_trip() {
        let action = DialAction { event: 2, unknown: 0, options: 1, action: 0x6D80 };
        let modes =
            DialModes { modes: vec![DialModeEntry { kind: 0x00, color: [0xFF, 0, 0, 0xFF], actions: vec![action] }] };
        let bytes = modes.encode();
        assert_eq!(bytes, [0x16, 0, 1, 0, 0x00, 0xFF, 0, 0, 0xFF, 1, 2, 0, 1, 0, 0x80, 0x6D]);
        assert_eq!(DialModes::decode(&bytes).unwrap(), modes);
    }
}
