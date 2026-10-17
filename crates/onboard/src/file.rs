use crate::wire::Reader;
use crate::{
    AssignmentMap, CommonLighting, DecodeError, DialModes, EffectKind, LightingsList, MacroAction, ProfileRoot,
    ProfilesList, RemapAction, StoredKeyMappings, TextAction,
};

const BLOB_HEADER: u16 = 0xB10B;
const BLOB_DESCRIPTOR_TYPE: u8 = 1;
const BLOB_DESCRIPTOR_SKIPPED_LEN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredFile {
    ProfilesList(ProfilesList),
    Root(ProfileRoot),
    AssignmentMap(AssignmentMap),
    KeyMappings(StoredKeyMappings),
    DialModes(DialModes),
    LightingsList(LightingsList),
    Lighting(CommonLighting),
    Remap(RemapAction),
    Macro(MacroAction),
    Text(TextAction),
    BlobDescriptor { data: Vec<u16> },
    Other { header: u16 },
}

pub fn decode(bytes: &[u8]) -> Result<StoredFile, DecodeError> {
    let header = match bytes {
        [low, high, ..] => u16::from_le_bytes([*low, *high]),
        _ => return Err(DecodeError(format!("a stored file needs a 2 byte header, got {} bytes", bytes.len()))),
    };
    Ok(match header {
        ProfilesList::HEADER => StoredFile::ProfilesList(ProfilesList::decode(bytes)?),
        ProfileRoot::HEADER => StoredFile::Root(ProfileRoot::decode(bytes)?),
        AssignmentMap::HEADER => StoredFile::AssignmentMap(AssignmentMap::decode(bytes)?),
        StoredKeyMappings::HEADER => StoredFile::KeyMappings(StoredKeyMappings::decode(bytes)?),
        DialModes::HEADER => StoredFile::DialModes(DialModes::decode(bytes)?),
        LightingsList::HEADER => StoredFile::LightingsList(LightingsList::decode(bytes)?),
        RemapAction::HEADER => StoredFile::Remap(RemapAction::decode(bytes)?),
        MacroAction::HEADER => StoredFile::Macro(MacroAction::decode(bytes)?),
        TextAction::HEADER => StoredFile::Text(TextAction::decode(bytes)?),
        BLOB_HEADER => decode_blob(bytes)?,
        other if EffectKind::from_header(other).is_some() => StoredFile::Lighting(CommonLighting::decode(bytes)?),
        other => StoredFile::Other { header: other },
    })
}

fn decode_blob(bytes: &[u8]) -> Result<StoredFile, DecodeError> {
    let mut reader = Reader::new(bytes, BLOB_HEADER)?;
    if reader.u8()? != BLOB_DESCRIPTOR_TYPE {
        return Ok(StoredFile::Other { header: BLOB_HEADER });
    }
    reader.take(BLOB_DESCRIPTOR_SKIPPED_LEN)?;
    let count = reader.u8()?;
    let data = (0..count).map(|_| reader.u16()).collect::<Result<_, _>>()?;
    Ok(StoredFile::BlobDescriptor { data })
}

impl StoredFile {
    pub fn children(&self) -> Vec<u16> {
        let ids: Vec<u16> = match self {
            Self::Root(root) => root.file_ids(),
            Self::AssignmentMap(map) => map.entries.iter().map(|entry| entry.action).collect(),
            Self::LightingsList(list) => list.entries.iter().map(|entry| entry.file).collect(),
            Self::DialModes(dials) => {
                dials.modes.iter().flat_map(|mode| &mode.actions).map(|action| action.action).collect()
            }
            Self::BlobDescriptor { data } => data.clone(),
            _ => Vec::new(),
        };
        ids.into_iter().filter(|id| *id != 0).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{BLOB_STAND_IN, file};

    #[test]
    fn decode_dispatches_on_the_header_and_lists_children() {
        assert_eq!(decode(file(0x6D70)).unwrap().children(), [0x6D69, 0x6D6D, 0x6D61, 0x6D6C, 0x6D6A, 0x6D6E]);
        assert_eq!(decode(file(0x6D69)).unwrap().children(), (0x6D62..=0x6D68).collect::<Vec<u16>>());
        assert_eq!(decode(file(0x6D6C)).unwrap().children(), [0x6D6B]);
        assert_eq!(decode(file(0x6D61)).unwrap().children(), [0x6D60]);
        assert!(decode(file(0x6D6E)).unwrap().children().is_empty());
        assert!(matches!(decode(&BLOB_STAND_IN).unwrap(), StoredFile::Other { header: 0xB10B }));
        assert!(matches!(decode(&[0x87, 0x00, 0x01]).unwrap(), StoredFile::Other { header: 0x0087 }));
        assert!(matches!(decode(file(0x6D6D)).unwrap(), StoredFile::Other { header: 0xD43D }));
        assert!(decode(&[0x1E]).is_err());
    }

    #[test]
    fn every_fixture_decodes_to_its_layout() {
        assert!(matches!(decode(file(0x0F)).unwrap(), StoredFile::ProfilesList(_)));
        assert!(matches!(decode(file(0x6D70)).unwrap(), StoredFile::Root(_)));
        assert!(matches!(decode(file(0x6D69)).unwrap(), StoredFile::AssignmentMap(_)));
        assert!(matches!(decode(file(0x6D6A)).unwrap(), StoredFile::KeyMappings(_)));
        assert!(matches!(decode(file(0x6D6E)).unwrap(), StoredFile::DialModes(_)));
        assert!(matches!(decode(file(0x6D6C)).unwrap(), StoredFile::LightingsList(_)));
        assert!(matches!(decode(file(0x6D6B)).unwrap(), StoredFile::Lighting(_)));
        assert!(matches!(decode(file(0x6D62)).unwrap(), StoredFile::Remap(_)));
        assert!(matches!(decode(file(0x6D61)).unwrap(), StoredFile::BlobDescriptor { .. }));
    }

    #[test]
    fn macro_and_text_files_decode_and_have_no_children() {
        let macro_file = crate::MacroAction { events: vec![crate::MacroEvent::KeyDown(4)] }.encode();
        assert!(matches!(decode(&macro_file).unwrap(), StoredFile::Macro(_)));
        let text_file = crate::TextAction::new(b"a".to_vec()).encode();
        let text = decode(&text_file).unwrap();
        assert!(matches!(text, StoredFile::Text(_)));
        assert!(text.children().is_empty());
    }

    #[test]
    fn dial_modes_list_their_action_files() {
        let mut mode = crate::DialModeEntry::new(1, k100::Rgb::new(1, 2, 3));
        mode.actions.push(crate::DialAction { event: 0, unknown: 0, options: 0, action: 0x6D80 });
        mode.actions.push(crate::DialAction { event: 1, unknown: 0, options: 0, action: 0 });
        let bytes = crate::DialModes { modes: vec![mode] }.encode();
        assert_eq!(decode(&bytes).unwrap().children(), [0x6D80]);
    }

    #[test]
    fn a_blob_descriptor_with_a_missing_id_is_a_decode_error() {
        assert!(decode(&[0x0B, 0xB1, 0x01, 1, 0, 0, 0, 0, 2, 0x60]).is_err());
    }
}
