use std::ops::BitOr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapping(pub u8);

impl Mapping {
    pub const NO_REPORT: Self = Self(0x00);
    pub const NORMAL: Self = Self(0x01);
    pub const WITH_FN: Self = Self(0x02);
    pub const WITH_FN2: Self = Self(0x04);
    pub const WITH_CTRL: Self = Self(0x08);
    pub const WITH_ALT: Self = Self(0x10);
    pub const WITH_SHIFT: Self = Self(0x20);
}

impl BitOr for Mapping {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMappings {
    bytes: Vec<u8>,
}

impl KeyMappings {
    pub fn all_normal(key_count: usize) -> Self {
        Self { bytes: vec![Mapping::NORMAL.0; key_count] }
    }

    pub fn set(&mut self, key: u8, mapping: Mapping) {
        if let Some(byte) = self.bytes.get_mut(key as usize) {
            *byte = mapping.0;
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_normal_and_sets_bits() {
        let mut mappings = KeyMappings::all_normal(146);
        mappings.set(0x47, Mapping::NO_REPORT);
        mappings.set(4, Mapping::WITH_CTRL | Mapping::WITH_SHIFT);
        let bytes = mappings.as_bytes();
        assert_eq!(bytes.len(), 146);
        assert_eq!(bytes[0x47], 0x00);
        assert_eq!(bytes[4], 0x28);
        assert_eq!(bytes[5], 0x01);
    }

    #[test]
    fn ignores_keys_outside_the_table() {
        let mut mappings = KeyMappings::all_normal(10);
        mappings.set(200, Mapping::NO_REPORT);
        assert!(mappings.as_bytes().iter().all(|&b| b == 1));
    }
}
