use crate::PropertyId;

const PROPERTY_VALUE_CHANGE: u8 = 0x01;
const KEY_PRESS_STATE_CHANGE: u8 = 0x02;
const RELATIVE_PROPERTY_CHANGE: u8 = 0x05;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    PropertyValueChange { property: PropertyId, value: u32 },
    KeyPressStateChange { bitmap: Vec<u8> },
    RelativePropertyChange { property: PropertyId, delta: i16 },
    Other { id: u8, payload: Vec<u8> },
}

impl Notification {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (&id, payload) = bytes.get(1..)?.split_first()?;
        Some(match id {
            PROPERTY_VALUE_CHANGE => Self::PropertyValueChange {
                property: property_at(payload)?,
                value: u32::from_le_bytes(payload.get(2..6)?.try_into().ok()?),
            },
            KEY_PRESS_STATE_CHANGE => Self::KeyPressStateChange {
                bitmap: payload.to_vec(),
            },
            RELATIVE_PROPERTY_CHANGE => Self::RelativePropertyChange {
                property: property_at(payload)?,
                delta: i16::from_le_bytes(payload.get(2..4)?.try_into().ok()?),
            },
            id => Self::Other {
                id,
                payload: payload.to_vec(),
            },
        })
    }
}

fn property_at(payload: &[u8]) -> Option<PropertyId> {
    Some(PropertyId(u16::from_le_bytes(payload.get(..2)?.try_into().ok()?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_bitmap() {
        let mut report = vec![0x00, 0x02, 0b0001_0000, 0x00];
        report.resize(64, 0);
        let Some(Notification::KeyPressStateChange { bitmap }) = Notification::parse(&report) else {
            panic!("expected key bitmap");
        };
        assert_eq!(bitmap.len(), 62);
        assert_eq!(bitmap[0], 0b0001_0000);
    }

    #[test]
    fn dial_rotation_is_a_signed_relative_change() {
        let report = [0x00, 0x05, 0x60, 0x00, 0xFF, 0xFF];
        assert_eq!(
            Notification::parse(&report),
            Some(Notification::RelativePropertyChange {
                property: PropertyId::DIAL_RING_ROTATION,
                delta: -1
            })
        );
    }

    #[test]
    fn property_value_change() {
        let report = [0x00, 0x01, 0x02, 0x00, 0xE8, 0x03, 0x00, 0x00];
        assert_eq!(
            Notification::parse(&report),
            Some(Notification::PropertyValueChange {
                property: PropertyId::BRIGHTNESS,
                value: 1000
            })
        );
    }

    #[test]
    fn unknown_and_truncated() {
        assert_eq!(
            Notification::parse(&[0x00, 0x09, 0x01]),
            Some(Notification::Other {
                id: 9,
                payload: vec![0x01]
            })
        );
        assert_eq!(Notification::parse(&[0x00]), None);
        assert_eq!(Notification::parse(&[0x00, 0x05, 0x60]), None);
    }
}
