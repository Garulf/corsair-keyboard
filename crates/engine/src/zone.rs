use k100::{LED_COUNT, led};
use serde::{Deserialize, Serialize};

use crate::layout::{Layout, LedKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedZone {
    All,
    Keys,
    Wasd,
    Numpad,
    Arrows,
    GKeys,
    Media,
    LightBar,
    Underglow,
    DialRing,
    Logo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Zone {
    Named(NamedZone),
    Leds(Vec<u8>),
}

pub type Mask = [bool; LED_COUNT];

impl Zone {
    pub fn mask(&self, layout: &Layout) -> Mask {
        let mut mask = [false; LED_COUNT];
        for geometry in layout.leds() {
            mask[geometry.index as usize] = self.contains(geometry.index, geometry.kind);
        }
        mask
    }

    fn contains(&self, index: u8, kind: LedKind) -> bool {
        match self {
            Zone::Leds(list) => list.contains(&index),
            Zone::Named(named) => match named {
                NamedZone::All => true,
                NamedZone::Keys => kind == LedKind::Key,
                NamedZone::Wasd => [4, 7, 22, 26].contains(&index),
                NamedZone::Numpad => (83..=99).contains(&index),
                NamedZone::Arrows => (79..=82).contains(&index),
                NamedZone::GKeys => (led::G1..=led::G6).contains(&index),
                NamedZone::Media => index == led::MUTE || (led::MEDIA_STOP..=led::MEDIA_PREVIOUS).contains(&index),
                NamedZone::LightBar => kind == LedKind::LightBar,
                NamedZone::Underglow => kind == LedKind::Underglow,
                NamedZone::DialRing => kind == LedKind::DialRing,
                NamedZone::Logo => kind == LedKind::Logo,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(zone: Zone) -> Vec<u8> {
        let mask = zone.mask(&Layout::ansi());
        (0..LED_COUNT as u8).filter(|&i| mask[i as usize]).collect()
    }

    #[test]
    fn named_zones() {
        assert_eq!(members(Zone::Named(NamedZone::Wasd)), [4, 7, 22, 26]);
        assert_eq!(members(Zone::Named(NamedZone::GKeys)), (131..=136).collect::<Vec<u8>>());
        assert_eq!(members(Zone::Named(NamedZone::Arrows)), [79, 80, 81, 82]);
        assert_eq!(members(Zone::Named(NamedZone::All)).len(), 177);
        assert_eq!(members(Zone::Named(NamedZone::Keys)).len(), 104 + 6 + 9 + 1);
    }

    #[test]
    fn explicit_lists_ignore_unknown_leds() {
        assert_eq!(members(Zone::Leds(vec![4, 100, 250])), [4]);
    }
}
