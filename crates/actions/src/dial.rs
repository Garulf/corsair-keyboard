use std::time::Duration;

use k100::led;

use crate::config::{DialConfig, DialMode, DialModeKind};
use crate::intent::{BrightnessChange, Intent, one_shot};
use crate::keys::Key;
use crate::program::{Program, tap};
use crate::recorder::{Recorder, RecorderState};
use crate::sink::{Axis, SinkEvent};
use crate::worker::OutputCommand;

pub const LONG_PRESS: Duration = Duration::from_millis(500);
pub const SWITCH_APPS_HOLD: Duration = Duration::from_millis(800);

pub struct Dial {
    modes: Vec<DialMode>,
    index: usize,
    pressed_at: Option<Duration>,
    long_fired: bool,
    alt_release_at: Option<Duration>,
}

impl Dial {
    pub fn new(config: &DialConfig) -> Self {
        Self { modes: config.modes.clone(), index: 0, pressed_at: None, long_fired: false, alt_release_at: None }
    }

    pub fn reconfigure(&mut self, config: &DialConfig) {
        self.modes = config.modes.clone();
        if self.index >= self.modes.len() {
            self.index = 0;
        }
        self.alt_release_at = None;
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn mode(&self) -> &DialMode {
        &self.modes[self.index]
    }

    pub fn press(&mut self, now: Duration) -> Vec<Intent> {
        self.pressed_at = Some(now);
        self.long_fired = false;
        self.release_alt()
    }

    pub fn release(&mut self, recorder: &mut Recorder) -> Vec<Intent> {
        let short = self.pressed_at.take().is_some() && !self.long_fired;
        if !short {
            return Vec::new();
        }
        if recorder.cancel() {
            return vec![Intent::Recording(RecorderState::Idle)];
        }
        self.index = (self.index + 1) % self.modes.len();
        vec![Intent::DialMode { index: self.index }]
    }

    pub fn rotate(&mut self, clockwise: bool, now: Duration) -> Vec<Intent> {
        let kind = self.mode().kind;
        let pick = |cw: &str, ccw: &str| key(if clockwise { cw } else { ccw });
        match kind {
            DialModeKind::Brightness => {
                vec![Intent::Brightness(if clockwise { BrightnessChange::Up } else { BrightnessChange::Down })]
            }
            DialModeKind::MacroRecording => Vec::new(),
            DialModeKind::MediaJogging => emit(tap(pick("right", "left"), &[])),
            DialModeKind::TrackSelector => emit(tap(pick("next_track", "previous_track"), &[])),
            DialModeKind::VerticalScroll => {
                emit(vec![SinkEvent::Wheel { axis: Axis::Vertical, clicks: if clockwise { -1 } else { 1 } }])
            }
            DialModeKind::HorizontalScroll => {
                emit(vec![SinkEvent::Wheel { axis: Axis::Horizontal, clicks: if clockwise { 1 } else { -1 } }])
            }
            DialModeKind::Zoom => emit(vec![
                SinkEvent::key(key("left_ctrl"), true),
                SinkEvent::Wheel { axis: Axis::Vertical, clicks: if clockwise { 1 } else { -1 } },
                SinkEvent::key(key("left_ctrl"), false),
            ]),
            DialModeKind::SwitchApps => {
                let mut intents = Vec::new();
                if self.alt_release_at.is_none() {
                    intents.push(Intent::Output(OutputCommand::Press {
                        source: led::DIAL_KEY,
                        program: Program::remap(key("left_alt"), &[]),
                    }));
                }
                let shift: &[Key] = if clockwise { &[] } else { &[key("left_shift")] };
                intents.extend(emit(tap(key("tab"), shift)));
                self.alt_release_at = Some(now + SWITCH_APPS_HOLD);
                intents
            }
            DialModeKind::Custom => {
                let mode = self.mode();
                let action = if clockwise { &mode.clockwise } else { &mode.counter_clockwise };
                action.as_ref().map(|action| one_shot(action, led::DIAL_KEY)).unwrap_or_default()
            }
        }
    }

    pub fn poll(&mut self, now: Duration, recorder: &mut Recorder) -> Vec<Intent> {
        let mut intents = Vec::new();
        if self.alt_release_at.is_some_and(|at| at <= now) {
            intents.extend(self.release_alt());
        }
        let held_long = self.pressed_at.is_some_and(|at| now.saturating_sub(at) >= LONG_PRESS);
        if held_long && !self.long_fired {
            self.long_fired = true;
            intents.extend(self.long_press(recorder));
        }
        intents
    }

    fn long_press(&mut self, recorder: &mut Recorder) -> Vec<Intent> {
        if recorder.cancel() {
            return vec![Intent::Recording(RecorderState::Idle)];
        }
        match self.mode().kind {
            DialModeKind::MacroRecording => {
                recorder.arm();
                vec![Intent::Recording(recorder.state())]
            }
            DialModeKind::TrackSelector => emit(tap(key("play_pause"), &[])),
            DialModeKind::Custom => {
                self.mode().press.as_ref().map(|action| one_shot(action, led::DIAL_KEY)).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn release_alt(&mut self) -> Vec<Intent> {
        match self.alt_release_at.take() {
            Some(_) => vec![Intent::Output(OutputCommand::Release { source: led::DIAL_KEY })],
            None => Vec::new(),
        }
    }
}

fn emit(events: Vec<SinkEvent>) -> Vec<Intent> {
    vec![Intent::Output(OutputCommand::Emit(events))]
}

fn key(name: &str) -> Key {
    Key::named(name).expect("built-in key names are in the key table")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, DialConfig, DialMode, DialModeKind};
    use crate::intent::ProfileTarget;
    use crate::keys::Key;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn dial_with(kinds: &[DialModeKind]) -> Dial {
        Dial::new(&DialConfig { modes: kinds.iter().copied().map(DialMode::builtin).collect() })
    }

    fn emitted(intents: Vec<Intent>) -> Vec<SinkEvent> {
        intents
            .into_iter()
            .flat_map(|intent| match intent {
                Intent::Output(OutputCommand::Emit(events)) => events,
                other => panic!("expected emitted events, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn short_presses_cycle_modes_and_wrap() {
        let mut dial = Dial::new(&DialConfig::default());
        let mut recorder = Recorder::default();
        for expected in [1, 2, 3, 0] {
            dial.press(ms(0));
            assert_eq!(dial.release(&mut recorder), [Intent::DialMode { index: expected }]);
        }
        assert_eq!(dial.mode().kind, DialModeKind::Brightness);
    }

    #[test]
    fn rotation_follows_the_mode() {
        let mut dial = dial_with(&[DialModeKind::Brightness]);
        assert_eq!(dial.rotate(true, ms(0)), [Intent::Brightness(BrightnessChange::Up)]);
        assert_eq!(dial.rotate(false, ms(0)), [Intent::Brightness(BrightnessChange::Down)]);
        let mut tracks = dial_with(&[DialModeKind::TrackSelector]);
        assert_eq!(emitted(tracks.rotate(true, ms(0))), tap(key("next_track"), &[]));
        let mut jog = dial_with(&[DialModeKind::MediaJogging]);
        assert_eq!(emitted(jog.rotate(false, ms(0))), tap(key("left"), &[]));
        let mut scroll = dial_with(&[DialModeKind::VerticalScroll]);
        assert_eq!(emitted(scroll.rotate(true, ms(0))), [SinkEvent::Wheel { axis: Axis::Vertical, clicks: -1 }]);
        let mut zoom = dial_with(&[DialModeKind::Zoom]);
        assert_eq!(
            emitted(zoom.rotate(true, ms(0))),
            [
                SinkEvent::key(key("left_ctrl"), true),
                SinkEvent::Wheel { axis: Axis::Vertical, clicks: 1 },
                SinkEvent::key(key("left_ctrl"), false),
            ]
        );
        assert!(dial_with(&[DialModeKind::MacroRecording]).rotate(true, ms(0)).is_empty());
    }

    #[test]
    fn a_long_press_arms_recording_and_its_release_keeps_it() {
        let mut dial = dial_with(&[DialModeKind::MacroRecording]);
        let mut recorder = Recorder::default();
        dial.press(ms(0));
        assert!(dial.poll(ms(499), &mut recorder).is_empty());
        assert_eq!(dial.poll(ms(500), &mut recorder), [Intent::Recording(RecorderState::AwaitingKey)]);
        assert!(dial.poll(ms(900), &mut recorder).is_empty(), "fires once");
        assert!(dial.release(&mut recorder).is_empty());
        assert_eq!(recorder.state(), RecorderState::AwaitingKey);
    }

    #[test]
    fn dial_presses_cancel_recording() {
        let mut dial = Dial::new(&DialConfig::default());
        let mut recorder = Recorder::default();
        recorder.arm();
        dial.press(ms(0));
        assert_eq!(dial.release(&mut recorder), [Intent::Recording(RecorderState::Idle)]);
        assert_eq!(dial.index(), 0, "the cancelling press does not change mode");
        recorder.arm();
        dial.press(ms(1000));
        assert_eq!(dial.poll(ms(1600), &mut recorder), [Intent::Recording(RecorderState::Idle)]);
    }

    #[test]
    fn track_selector_long_press_toggles_playback() {
        let mut dial = dial_with(&[DialModeKind::TrackSelector]);
        let mut recorder = Recorder::default();
        dial.press(ms(0));
        assert_eq!(emitted(dial.poll(ms(600), &mut recorder)), tap(key("play_pause"), &[]));
    }

    fn alt_press() -> Intent {
        Intent::Output(OutputCommand::Press { source: 137, program: Program::remap(key("left_alt"), &[]) })
    }

    fn alt_release() -> Intent {
        Intent::Output(OutputCommand::Release { source: 137 })
    }

    fn tab_tap(modifiers: &[Key]) -> Intent {
        Intent::Output(OutputCommand::Emit(tap(key("tab"), modifiers)))
    }

    #[test]
    fn switch_apps_holds_alt_until_the_dial_rests() {
        let mut dial = dial_with(&[DialModeKind::SwitchApps]);
        let mut recorder = Recorder::default();
        assert_eq!(dial.rotate(true, ms(0)), [alt_press(), tab_tap(&[])]);
        assert_eq!(dial.rotate(false, ms(300)), [tab_tap(&[key("left_shift")])]);
        assert!(dial.poll(ms(1099), &mut recorder).is_empty());
        assert_eq!(dial.poll(ms(1100), &mut recorder), [alt_release()]);
        assert!(dial.poll(ms(1200), &mut recorder).is_empty());
    }

    #[test]
    fn a_dial_press_releases_a_held_alt() {
        let mut dial = dial_with(&[DialModeKind::SwitchApps]);
        dial.rotate(true, ms(0));
        assert_eq!(dial.press(ms(100)), [alt_release()]);
    }

    #[test]
    fn reconfigure_forgets_a_held_alt() {
        let mut dial = dial_with(&[DialModeKind::SwitchApps]);
        let mut recorder = Recorder::default();
        dial.rotate(true, ms(0));
        dial.reconfigure(&DialConfig { modes: vec![DialMode::builtin(DialModeKind::SwitchApps)] });
        assert!(dial.poll(ms(2000), &mut recorder).is_empty());
    }

    #[test]
    fn custom_modes_run_their_actions() {
        let volume = DialMode {
            clockwise: Some(Action::Remap { key: key("volume_up"), modifiers: vec![] }),
            press: Some(Action::NextProfile),
            ..DialMode::builtin(DialModeKind::Custom)
        };
        let mut dial = Dial::new(&DialConfig { modes: vec![volume] });
        let mut recorder = Recorder::default();
        assert_eq!(
            dial.rotate(true, ms(0)),
            [
                Intent::Output(OutputCommand::Press { source: 137, program: Program::remap(key("volume_up"), &[]) }),
                Intent::Output(OutputCommand::Release { source: 137 }),
            ]
        );
        assert!(dial.rotate(false, ms(0)).is_empty());
        dial.press(ms(0));
        assert_eq!(dial.poll(ms(500), &mut recorder), [Intent::SwitchProfile(ProfileTarget::Next)]);
    }

    #[test]
    fn reconfigure_keeps_a_valid_index() {
        let mut dial = Dial::new(&DialConfig::default());
        let mut recorder = Recorder::default();
        for _ in 0..3 {
            dial.press(ms(0));
            dial.release(&mut recorder);
        }
        dial.reconfigure(&DialConfig { modes: vec![DialMode::builtin(DialModeKind::Zoom)] });
        assert_eq!(dial.index(), 0);
        dial.reconfigure(&DialConfig::default());
        assert_eq!(dial.index(), 0);
    }
}
