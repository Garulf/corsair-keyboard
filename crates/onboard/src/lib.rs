use std::ops::RangeInclusive;

mod action;
mod convert;
mod file;
mod lighting;
mod mappings;
mod root;
mod wire;

#[cfg(test)]
mod fixtures;

pub use action::{MacroAction, MacroEvent, RemapAction, RemapTarget, TextAction, execution};
pub use convert::{MAX_EFFECTS, Report, convert_layers};
pub use file::{StoredFile, decode as decode_file};
pub use lighting::{
    CommonLighting, Direction, EffectDuration, EffectKind, HwColor, LightingEntry, LightingsList, SPECIFIED_COLORS,
    START_WITH_PROFILE, Speed,
};
pub use mappings::{AssignmentEntry, AssignmentMap, DialAction, DialModeEntry, DialModes, StoredKeyMappings};
pub use root::{ProfileRoot, ProfilesList, Section};

pub const KEY_COUNT: usize = 138;
pub const FREE_USE_IDS: RangeInclusive<u16> = 0x6D60..=0x7FFF;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DecodeError(pub String);
