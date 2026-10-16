use std::collections::{HashMap, HashSet};
use std::time::Duration;

use k100::{InputEvent, led};

use crate::config::{Assignment, DialConfig, DialMode};
use crate::dial::Dial;
use crate::intent::{Intent, ProfileTarget, action_intents};
use crate::recorder::{Recorder, RecorderOutcome, RecorderState};

pub struct Dispatcher {
    bindings: HashMap<u8, Assignment>,
    recorder: Recorder,
    dial: Dial,
    held: HashSet<u8>,
    firing: HashSet<u8>,
}

impl Dispatcher {
    pub fn new(assignments: &[Assignment], dial: &DialConfig) -> Self {
        Self {
            bindings: bind(assignments),
            recorder: Recorder::default(),
            dial: Dial::new(dial),
            held: HashSet::new(),
            firing: HashSet::new(),
        }
    }

    pub fn reconfigure(&mut self, assignments: &[Assignment], dial: &DialConfig) {
        self.bindings = bind(assignments);
        self.dial.reconfigure(dial);
        self.firing.clear();
    }

    pub fn forget_held_keys(&mut self) {
        self.held.clear();
        self.firing.clear();
    }

    pub fn is_busy(&self) -> bool {
        !self.firing.is_empty() || self.dial.holds_alt()
    }

    pub fn cancel_recording(&mut self) -> bool {
        self.recorder.cancel()
    }

    pub fn recorder_state(&self) -> RecorderState {
        self.recorder.state()
    }

    pub fn arm_recording(&mut self) -> RecorderState {
        self.recorder.arm();
        self.recorder.state()
    }

    pub fn dial_mode(&self) -> &DialMode {
        self.dial.mode()
    }

    pub fn dial_index(&self) -> usize {
        self.dial.index()
    }

    pub fn handle(&mut self, event: &InputEvent, now: Duration) -> Vec<Intent> {
        match *event {
            InputEvent::KeyDown(index) => self.key_down(index, now),
            InputEvent::KeyUp(index) => self.key_up(index, now),
            InputEvent::DialCw => self.dial.rotate(true, now),
            InputEvent::DialCcw => self.dial.rotate(false, now),
            InputEvent::Property { .. } => Vec::new(),
        }
    }

    pub fn poll(&mut self, now: Duration) -> Vec<Intent> {
        self.dial.poll(now, &mut self.recorder)
    }

    fn key_down(&mut self, index: u8, now: Duration) -> Vec<Intent> {
        self.held.insert(index);
        if index == led::DIAL_KEY {
            return self.dial.press(now);
        }
        if let Some(intents) = self.recorder_intents(index, true, now) {
            return intents;
        }
        if index == led::PROFILE && self.recorder.cancel() {
            return vec![Intent::Recording(RecorderState::Idle)];
        }
        if let Some(assignment) = self.bindings.get(&index) {
            let native = assignment
                .pass_through
                .iter()
                .any(|pass| pass.held_by().iter().any(|modifier| self.held.contains(modifier)));
            if native {
                return Vec::new();
            }
            self.firing.insert(index);
            return action_intents(&assignment.action, index, true);
        }
        if index == led::PROFILE {
            return vec![Intent::SwitchProfile(ProfileTarget::Next)];
        }
        Vec::new()
    }

    fn key_up(&mut self, index: u8, now: Duration) -> Vec<Intent> {
        self.held.remove(&index);
        if index == led::DIAL_KEY {
            let mut intents = self.dial.poll(now, &mut self.recorder);
            intents.extend(self.dial.release(&mut self.recorder));
            return intents;
        }
        let mut intents = match (self.firing.remove(&index), self.bindings.get(&index)) {
            (true, Some(assignment)) => action_intents(&assignment.action, index, false),
            _ => Vec::new(),
        };
        intents.extend(self.recorder_intents(index, false, now).unwrap_or_default());
        intents
    }

    fn recorder_intents(&mut self, index: u8, down: bool, now: Duration) -> Option<Vec<Intent>> {
        match self.recorder.key(index, down, now) {
            RecorderOutcome::Pass => None,
            RecorderOutcome::Consumed => Some(Vec::new()),
            RecorderOutcome::Started { .. } => Some(vec![Intent::Recording(self.recorder.state())]),
            RecorderOutcome::Finished { key, steps } => {
                Some(vec![Intent::SaveMacro { key, steps }, Intent::Recording(RecorderState::Idle)])
            }
            RecorderOutcome::Cancelled => Some(vec![Intent::Recording(RecorderState::Idle)]),
        }
    }
}

fn bind(assignments: &[Assignment]) -> HashMap<u8, Assignment> {
    assignments.iter().filter_map(|a| a.key.index().map(|index| (index, a.clone()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, DialMode, DialModeKind, MacroStep, PassThrough};
    use crate::intent::BrightnessChange;
    use crate::keys::Key;
    use crate::program::Program;
    use crate::worker::OutputCommand;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn assign(name: &str, action: Action) -> Assignment {
        Assignment { key: key(name), action, pass_through: Vec::new() }
    }

    fn escape_on_caps() -> Assignment {
        assign("caps_lock", Action::Remap { key: key("escape"), modifiers: vec![] })
    }

    fn macro_dial() -> DialConfig {
        DialConfig { modes: vec![DialMode::builtin(DialModeKind::MacroRecording)] }
    }

    fn arm(dispatcher: &mut Dispatcher, at: u64) {
        dispatcher.handle(&InputEvent::KeyDown(137), ms(at));
        dispatcher.poll(ms(at + 500));
        dispatcher.handle(&InputEvent::KeyUp(137), ms(at + 600));
        assert_eq!(dispatcher.recorder_state(), RecorderState::AwaitingKey);
    }

    #[test]
    fn arming_from_outside_makes_the_next_g_key_the_target() {
        let mut dispatcher = Dispatcher::new(&[], &DialConfig::default());
        assert_eq!(dispatcher.arm_recording(), RecorderState::AwaitingKey);
        let intents = dispatcher.handle(&InputEvent::KeyDown(led::G1), Duration::ZERO);
        assert_eq!(intents, [Intent::Recording(RecorderState::Recording { key: led::G1 })]);
    }

    #[test]
    fn assigned_keys_press_and_release_their_action() {
        let mut dispatcher = Dispatcher::new(&[escape_on_caps()], &DialConfig::default());
        let program = Program::remap(key("escape"), &[]);
        assert_eq!(
            dispatcher.handle(&InputEvent::KeyDown(57), ms(0)),
            [Intent::Output(OutputCommand::Press { source: 57, program })]
        );
        assert_eq!(
            dispatcher.handle(&InputEvent::KeyUp(57), ms(10)),
            [Intent::Output(OutputCommand::Release { source: 57 })]
        );
        assert!(dispatcher.handle(&InputEvent::KeyDown(4), ms(20)).is_empty());
    }

    #[test]
    fn pass_through_modifiers_leave_the_key_native() {
        let mut caps = escape_on_caps();
        caps.pass_through = vec![PassThrough::Shift];
        let mut dispatcher = Dispatcher::new(&[caps], &DialConfig::default());
        dispatcher.handle(&InputEvent::KeyDown(106), ms(0));
        assert!(dispatcher.handle(&InputEvent::KeyDown(57), ms(1)).is_empty());
        assert!(dispatcher.handle(&InputEvent::KeyUp(57), ms(2)).is_empty());
        dispatcher.handle(&InputEvent::KeyUp(106), ms(3));
        assert_eq!(dispatcher.handle(&InputEvent::KeyDown(57), ms(4)).len(), 1);
    }

    #[test]
    fn the_profile_key_switches_profiles_unless_assigned() {
        let mut plain = Dispatcher::new(&[], &DialConfig::default());
        assert_eq!(plain.handle(&InputEvent::KeyDown(128), ms(0)), [Intent::SwitchProfile(ProfileTarget::Next)]);
        let mut assigned = Dispatcher::new(&[assign("profile", Action::PreviousProfile)], &DialConfig::default());
        assert_eq!(assigned.handle(&InputEvent::KeyDown(128), ms(0)), [Intent::SwitchProfile(ProfileTarget::Previous)]);
    }

    #[test]
    fn recording_a_macro_through_the_dial() {
        let mut dispatcher = Dispatcher::new(&[], &DialConfig::default());
        dispatcher.handle(&InputEvent::KeyDown(137), ms(0));
        assert_eq!(dispatcher.handle(&InputEvent::KeyUp(137), ms(50)), [Intent::DialMode { index: 1 }]);
        dispatcher.handle(&InputEvent::KeyDown(137), ms(100));
        assert_eq!(dispatcher.poll(ms(600)), [Intent::Recording(RecorderState::AwaitingKey)]);
        assert!(dispatcher.handle(&InputEvent::KeyUp(137), ms(700)).is_empty());
        assert_eq!(
            dispatcher.handle(&InputEvent::KeyDown(131), ms(800)),
            [Intent::Recording(RecorderState::Recording { key: 131 })]
        );
        assert!(dispatcher.handle(&InputEvent::KeyUp(131), ms(820)).is_empty());
        assert!(dispatcher.handle(&InputEvent::KeyDown(4), ms(900)).is_empty());
        assert!(dispatcher.handle(&InputEvent::KeyUp(4), ms(950)).is_empty());
        let steps = vec![MacroStep::Down(key("a")), MacroStep::Delay(50), MacroStep::Up(key("a"))];
        assert_eq!(
            dispatcher.handle(&InputEvent::KeyDown(131), ms(1000)),
            [Intent::SaveMacro { key: key("g1"), steps }, Intent::Recording(RecorderState::Idle),]
        );
    }

    #[test]
    fn the_profile_key_cancels_recording_instead_of_switching() {
        let mut dispatcher = Dispatcher::new(&[], &macro_dial());
        arm(&mut dispatcher, 0);
        assert_eq!(dispatcher.handle(&InputEvent::KeyDown(128), ms(1000)), [Intent::Recording(RecorderState::Idle)]);
    }

    #[test]
    fn the_g_key_that_finishes_a_recording_does_not_fire_its_old_action() {
        let mut dispatcher = Dispatcher::new(&[assign("g1", Action::Text { text: "x".into() })], &macro_dial());
        arm(&mut dispatcher, 0);
        dispatcher.handle(&InputEvent::KeyDown(131), ms(1000));
        dispatcher.handle(&InputEvent::KeyUp(131), ms(1010));
        dispatcher.handle(&InputEvent::KeyDown(4), ms(1100));
        dispatcher.handle(&InputEvent::KeyUp(4), ms(1150));
        let finish = dispatcher.handle(&InputEvent::KeyDown(131), ms(1200));
        assert!(matches!(finish[0], Intent::SaveMacro { .. }), "{finish:?}");
        assert!(dispatcher.handle(&InputEvent::KeyUp(131), ms(1210)).is_empty());
    }

    #[test]
    fn a_long_dial_hold_released_between_polls_still_arms_recording() {
        let mut dispatcher = Dispatcher::new(&[], &macro_dial());
        dispatcher.handle(&InputEvent::KeyDown(137), ms(0));
        assert_eq!(
            dispatcher.handle(&InputEvent::KeyUp(137), ms(600)),
            [Intent::Recording(RecorderState::AwaitingKey)]
        );
        assert_eq!(dispatcher.dial_index(), 0);
    }

    #[test]
    fn releasing_a_held_g_key_after_recording_is_armed_still_releases_its_action() {
        let mut dispatcher =
            Dispatcher::new(&[assign("g1", Action::Remap { key: key("escape"), modifiers: vec![] })], &macro_dial());
        let program = Program::remap(key("escape"), &[]);
        assert_eq!(
            dispatcher.handle(&InputEvent::KeyDown(131), ms(0)),
            [Intent::Output(OutputCommand::Press { source: 131, program })]
        );
        dispatcher.handle(&InputEvent::KeyDown(137), ms(10));
        assert_eq!(dispatcher.poll(ms(510)), [Intent::Recording(RecorderState::AwaitingKey)]);
        assert_eq!(
            dispatcher.handle(&InputEvent::KeyUp(131), ms(600)),
            [Intent::Output(OutputCommand::Release { source: 131 })]
        );
    }

    #[test]
    fn forgetting_held_keys_clears_stale_pass_through_modifiers() {
        let mut caps = escape_on_caps();
        caps.pass_through = vec![PassThrough::Shift];
        let mut dispatcher = Dispatcher::new(&[caps], &DialConfig::default());
        dispatcher.handle(&InputEvent::KeyDown(106), ms(0));
        assert!(dispatcher.handle(&InputEvent::KeyDown(57), ms(1)).is_empty());
        dispatcher.forget_held_keys();
        assert_eq!(dispatcher.handle(&InputEvent::KeyDown(57), ms(2)).len(), 1);
    }

    #[test]
    fn the_profile_key_cancels_an_active_recording() {
        let mut dispatcher = Dispatcher::new(&[], &macro_dial());
        arm(&mut dispatcher, 0);
        dispatcher.handle(&InputEvent::KeyDown(131), ms(1000));
        assert_eq!(dispatcher.recorder_state(), RecorderState::Recording { key: 131 });
        assert_eq!(dispatcher.handle(&InputEvent::KeyDown(128), ms(1100)), [Intent::Recording(RecorderState::Idle)]);
        assert_eq!(dispatcher.recorder_state(), RecorderState::Idle);
    }

    #[test]
    fn dial_turns_follow_the_mode() {
        let mut dispatcher = Dispatcher::new(&[], &DialConfig::default());
        assert_eq!(dispatcher.handle(&InputEvent::DialCw, ms(0)), [Intent::Brightness(BrightnessChange::Up)]);
        let property = InputEvent::Property { property: bragi::PropertyId::WIN_LOCK_ENABLED, value: 1 };
        assert!(dispatcher.handle(&property, ms(0)).is_empty());
    }

    #[test]
    fn busy_while_an_action_key_is_held_or_the_dial_holds_alt() {
        let mut dispatcher = Dispatcher::new(&[escape_on_caps()], &DialConfig::default());
        assert!(!dispatcher.is_busy());
        dispatcher.handle(&InputEvent::KeyDown(57), ms(0));
        assert!(dispatcher.is_busy());
        dispatcher.handle(&InputEvent::KeyUp(57), ms(10));
        assert!(!dispatcher.is_busy());
        let switcher = DialConfig { modes: vec![DialMode::builtin(DialModeKind::SwitchApps)] };
        let mut dial = Dispatcher::new(&[], &switcher);
        dial.handle(&InputEvent::DialCw, ms(0));
        assert!(dial.is_busy());
        dial.poll(ms(1000));
        assert!(!dial.is_busy());
    }

    #[test]
    fn cancel_recording_reports_whether_one_was_active() {
        let mut dispatcher = Dispatcher::new(&[], &macro_dial());
        assert!(!dispatcher.cancel_recording());
        arm(&mut dispatcher, 0);
        assert!(dispatcher.cancel_recording());
        assert_eq!(dispatcher.recorder_state(), RecorderState::Idle);
    }

    #[test]
    fn reconfigure_forgets_held_actions() {
        let mut dispatcher = Dispatcher::new(&[escape_on_caps()], &DialConfig::default());
        dispatcher.handle(&InputEvent::KeyDown(57), ms(0));
        dispatcher.reconfigure(&[], &DialConfig::default());
        assert!(dispatcher.handle(&InputEvent::KeyUp(57), ms(10)).is_empty());
    }
}
