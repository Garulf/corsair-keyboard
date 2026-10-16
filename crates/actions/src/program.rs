use std::sync::Arc;

use crate::config::{MacroStep, MouseButton, RepeatMode};
use crate::keys::Key;
use crate::sink::SinkEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    Hold { press: Vec<SinkEvent>, repeat: Option<SinkEvent> },
    Macro { steps: Arc<[MacroStep]>, mode: RepeatMode },
}

impl Program {
    pub fn remap(key: Key, modifiers: &[Key]) -> Self {
        let mut press: Vec<SinkEvent> = modifiers.iter().map(|modifier| SinkEvent::key(*modifier, true)).collect();
        press.push(SinkEvent::key(key, true));
        let repeat = (!key.is_modifier()).then(|| SinkEvent::key(key, true));
        Self::Hold { press, repeat }
    }

    pub fn mouse(button: MouseButton) -> Self {
        Self::Hold { press: vec![SinkEvent::Mouse { button, down: true }], repeat: None }
    }

    pub fn macro_steps(steps: &[MacroStep], mode: RepeatMode) -> Self {
        Self::Macro { steps: steps.into(), mode }
    }
}

pub fn tap(key: Key, modifiers: &[Key]) -> Vec<SinkEvent> {
    let mut events: Vec<SinkEvent> = modifiers.iter().map(|modifier| SinkEvent::key(*modifier, true)).collect();
    events.push(SinkEvent::key(key, true));
    events.push(SinkEvent::key(key, false));
    events.extend(modifiers.iter().rev().map(|modifier| SinkEvent::key(*modifier, false)));
    events
}
