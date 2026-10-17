use std::ops::RangeInclusive;

mod action;
mod backup;
mod convert;
mod error;
mod file;
mod lighting;
mod mappings;
mod memory;
mod root;
mod service;
mod store;
mod tree;
mod usage;
mod wire;

#[cfg(test)]
mod fixtures;

pub use action::{MacroAction, MacroEvent, RemapAction, RemapTarget, TextAction, execution};
pub use backup::{Backups, Snapshot, utc_stamp};
pub use convert::{
    ActionFile, Conversion, KeyAction, MAX_EFFECTS, Report, convert, convert_assignments, convert_dial, convert_layers,
};
pub use error::OnboardError;
pub use file::{StoredFile, children_of, decode as decode_file};
pub use lighting::{
    CommonLighting, Direction, EffectDuration, EffectKind, HwColor, LightingEntry, LightingsList, SPECIFIED_COLORS,
    START_WITH_PROFILE, Speed,
};
pub use mappings::{AssignmentEntry, AssignmentMap, DialAction, DialModeEntry, DialModes, StoredKeyMappings};
pub use memory::{Failure, MemoryStore, Operation};
pub use root::{ProfileRoot, ProfilesList, Section};
pub use service::execute;
pub use store::{
    Changed, FileStore, STORE_HANDLE, Saved, SlotList, SlotSummary, StoredTree, activate, delete, list, read_tree,
    replay_hardware_profile, restore, save, snapshot,
};
pub use tree::{FilePlan, OnboardTree};
pub use usage::{Usage, hid_usage};

pub const KEY_COUNT: usize = 138;
pub const FREE_USE_IDS: RangeInclusive<u16> = 0x6D60..=0x7FFF;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DecodeError(pub String);
