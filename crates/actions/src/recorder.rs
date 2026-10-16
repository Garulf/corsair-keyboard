use std::time::Duration;

use k100::led;
use serde::{Deserialize, Serialize};

use crate::config::MacroStep;
use crate::keys::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RecorderState {
    #[default]
    Idle,
    AwaitingKey,
    Recording {
        key: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderOutcome {
    Pass,
    Consumed,
    Started { key: u8 },
    Finished { key: Key, steps: Vec<MacroStep> },
    Cancelled,
}

#[derive(Debug, Default)]
pub struct Recorder {
    state: RecorderState,
    events: Vec<(Duration, Key, bool)>,
}

impl Recorder {
    pub fn state(&self) -> RecorderState {
        self.state
    }

    pub fn is_active(&self) -> bool {
        self.state != RecorderState::Idle
    }

    pub fn arm(&mut self) {
        self.state = RecorderState::AwaitingKey;
        self.events.clear();
    }

    pub fn cancel(&mut self) -> bool {
        let was_active = self.is_active();
        self.state = RecorderState::Idle;
        self.events.clear();
        was_active
    }

    pub fn key(&mut self, index: u8, down: bool, now: Duration) -> RecorderOutcome {
        match self.state {
            RecorderState::Idle => RecorderOutcome::Pass,
            RecorderState::AwaitingKey if is_g_key(index) => {
                if down {
                    self.state = RecorderState::Recording { key: index };
                    RecorderOutcome::Started { key: index }
                } else {
                    RecorderOutcome::Consumed
                }
            }
            RecorderState::AwaitingKey => RecorderOutcome::Pass,
            RecorderState::Recording { key } if key == index => {
                if !down {
                    return RecorderOutcome::Consumed;
                }
                let steps = build_steps(&self.events);
                self.cancel();
                match Key::from_index(index) {
                    Some(target) if !steps.is_empty() => RecorderOutcome::Finished { key: target, steps },
                    _ => RecorderOutcome::Cancelled,
                }
            }
            RecorderState::Recording { .. } => {
                if let Some(key) = Key::from_index(index).filter(|key| key.output().is_some()) {
                    self.events.push((now, key, down));
                }
                RecorderOutcome::Pass
            }
        }
    }
}

fn is_g_key(index: u8) -> bool {
    (led::G1..=led::G6).contains(&index)
}

fn build_steps(events: &[(Duration, Key, bool)]) -> Vec<MacroStep> {
    let mut steps = Vec::new();
    let mut down: Vec<Key> = Vec::new();
    let mut previous: Option<Duration> = None;
    for &(at, key, pressed) in events {
        if !pressed && !down.contains(&key) {
            continue;
        }
        if let Some(gap) = previous.map(|before| at.saturating_sub(before).as_millis())
            && gap > 0
        {
            steps.push(MacroStep::Delay(u32::try_from(gap).unwrap_or(u32::MAX)));
        }
        previous = Some(at);
        if pressed {
            if !down.contains(&key) {
                down.push(key);
            }
            steps.push(MacroStep::Down(key));
        } else {
            down.retain(|held| *held != key);
            steps.push(MacroStep::Up(key));
        }
    }
    steps.extend(down.into_iter().rev().map(MacroStep::Up));
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn records_between_two_presses_of_the_same_g_key() {
        let mut recorder = Recorder::default();
        recorder.arm();
        assert_eq!(recorder.state(), RecorderState::AwaitingKey);
        assert_eq!(recorder.key(4, true, at(0)), RecorderOutcome::Pass, "normal keys pass while waiting");
        assert_eq!(recorder.key(4, false, at(5)), RecorderOutcome::Pass);
        assert_eq!(recorder.key(132, true, at(10)), RecorderOutcome::Started { key: 132 });
        assert_eq!(recorder.state(), RecorderState::Recording { key: 132 });
        assert_eq!(recorder.key(132, false, at(20)), RecorderOutcome::Consumed);
        for (index, down, time) in [(105, true, 100), (6, true, 150), (6, false, 170), (105, false, 200)] {
            assert_eq!(recorder.key(index, down, at(time)), RecorderOutcome::Pass);
        }
        assert_eq!(recorder.key(131, true, at(210)), RecorderOutcome::Pass, "other G keys are not recorded");
        let steps = vec![
            MacroStep::Down(key("left_ctrl")),
            MacroStep::Delay(50),
            MacroStep::Down(key("c")),
            MacroStep::Delay(20),
            MacroStep::Up(key("c")),
            MacroStep::Delay(30),
            MacroStep::Up(key("left_ctrl")),
        ];
        assert_eq!(recorder.key(132, true, at(300)), RecorderOutcome::Finished { key: key("g2"), steps });
        assert_eq!(recorder.state(), RecorderState::Idle);
    }

    #[test]
    fn unmatched_releases_are_dropped_and_open_keys_closed() {
        let mut recorder = Recorder::default();
        recorder.arm();
        recorder.key(131, true, at(0));
        recorder.key(4, false, at(10));
        recorder.key(5, true, at(20));
        let steps = vec![MacroStep::Down(key("b")), MacroStep::Up(key("b"))];
        assert_eq!(recorder.key(131, true, at(50)), RecorderOutcome::Finished { key: key("g1"), steps });
    }

    #[test]
    fn finishing_without_keys_is_a_cancel() {
        let mut recorder = Recorder::default();
        recorder.arm();
        recorder.key(131, true, at(0));
        recorder.key(131, false, at(10));
        assert_eq!(recorder.key(131, true, at(20)), RecorderOutcome::Cancelled);
        assert_eq!(recorder.state(), RecorderState::Idle);
    }

    #[test]
    fn cancel_resets() {
        let mut recorder = Recorder::default();
        assert!(!recorder.cancel());
        recorder.arm();
        assert!(recorder.cancel());
        assert_eq!(recorder.state(), RecorderState::Idle);
        assert_eq!(recorder.key(131, true, at(0)), RecorderOutcome::Pass);
    }
}
