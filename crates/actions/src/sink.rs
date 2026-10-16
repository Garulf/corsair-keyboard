use std::sync::{Arc, Mutex};

use crate::config::MouseButton;
use crate::keys::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkEvent {
    Key { key: Key, down: bool },
    Mouse { button: MouseButton, down: bool },
    Wheel { axis: Axis, clicks: i32 },
    Text(String),
}

impl SinkEvent {
    pub fn key(key: Key, down: bool) -> Self {
        Self::Key { key, down }
    }

    pub fn release_of(&self) -> Option<SinkEvent> {
        match *self {
            Self::Key { key, down: true } => Some(Self::Key { key, down: false }),
            Self::Mouse { button, down: true } => Some(Self::Mouse { button, down: false }),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SinkError(pub String);

pub trait InputSink: Send {
    fn send(&mut self, event: &SinkEvent) -> Result<(), SinkError>;

    fn needs_typematic(&self) -> bool {
        false
    }
}

pub struct NullSink;

impl InputSink for NullSink {
    fn send(&mut self, _event: &SinkEvent) -> Result<(), SinkError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct RecordingSink {
    pub events: Arc<Mutex<Vec<SinkEvent>>>,
    pub typematic: bool,
}

impl RecordingSink {
    pub fn take(&self) -> Vec<SinkEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

impl InputSink for RecordingSink {
    fn send(&mut self, event: &SinkEvent) -> Result<(), SinkError> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }

    fn needs_typematic(&self) -> bool {
        self.typematic
    }
}
