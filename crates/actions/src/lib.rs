mod keys;

pub use keys::{Key, KeyDef, Output, all};

mod config;

pub use config::{
    Action, Assignment, DialConfig, DialMode, DialModeKind, MacroStep, MouseButton, PassThrough, RepeatMode,
    key_mappings, validate,
};

mod player;
mod program;
mod sink;

pub use player::{LOOP_GAP, Player, TYPEMATIC_DELAY, TYPEMATIC_INTERVAL};
pub use program::{Program, tap};
pub use sink::{Axis, InputSink, NullSink, RecordingSink, SinkError, SinkEvent};

mod worker;

pub use worker::{OutputCommand, OutputHandle, spawn};

mod platform;

pub use platform::platform_sink;
