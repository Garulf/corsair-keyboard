use std::sync::Arc;
use std::time::Duration;

use crate::config::{MacroStep, RepeatMode};
use crate::program::Program;
use crate::sink::{Axis, SinkEvent};

pub const TYPEMATIC_DELAY: Duration = Duration::from_millis(500);
pub const TYPEMATIC_INTERVAL: Duration = Duration::from_millis(33);
pub const LOOP_GAP: Duration = Duration::from_millis(10);

pub struct Player {
    typematic: bool,
    tasks: Vec<Task>,
}

struct Task {
    source: u8,
    held: Vec<SinkEvent>,
    due: Option<Duration>,
    body: Body,
}

enum Body {
    Hold { repeat: Option<SinkEvent> },
    Macro { steps: Arc<[MacroStep]>, mode: RepeatMode, position: usize },
}

impl Player {
    pub fn new(typematic: bool) -> Self {
        Self { typematic, tasks: Vec::new() }
    }

    pub fn is_idle(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn is_playing(&self, source: u8) -> bool {
        self.find(source).is_some()
    }

    pub fn press(&mut self, source: u8, program: Program, now: Duration) -> Vec<SinkEvent> {
        if let Some(index) = self.find(source) {
            let toggles_off = matches!(self.tasks[index].body, Body::Macro { mode: RepeatMode::Toggle, .. });
            return if toggles_off { self.stop(index) } else { Vec::new() };
        }
        match program {
            Program::Hold { press, repeat } => {
                let due = repeat.as_ref().filter(|_| self.typematic).map(|_| now + TYPEMATIC_DELAY);
                let held = press.iter().filter(|event| event.release_of().is_some()).cloned().collect();
                self.tasks.push(Task { source, held, due, body: Body::Hold { repeat } });
                press
            }
            Program::Macro { steps, mode } => {
                let body = Body::Macro { steps, mode, position: 0 };
                let mut task = Task { source, held: Vec::new(), due: Some(now), body };
                let mut out = Vec::new();
                if advance(&mut task, now, &mut out) {
                    out.extend(release_all(&mut task.held));
                } else {
                    self.tasks.push(task);
                }
                out
            }
        }
    }

    pub fn release(&mut self, source: u8, _now: Duration) -> Vec<SinkEvent> {
        let Some(index) = self.find(source) else { return Vec::new() };
        match self.tasks[index].body {
            Body::Hold { .. } | Body::Macro { mode: RepeatMode::WhileHeld, .. } => self.stop(index),
            Body::Macro { .. } => Vec::new(),
        }
    }

    pub fn cancel_all(&mut self) -> Vec<SinkEvent> {
        let mut out = Vec::new();
        for mut task in self.tasks.drain(..) {
            out.extend(release_all(&mut task.held));
        }
        out
    }

    pub fn poll(&mut self, now: Duration) -> Vec<SinkEvent> {
        let mut out = Vec::new();
        let mut index = 0;
        while index < self.tasks.len() {
            let task = &mut self.tasks[index];
            let due = task.due.is_some_and(|due| due <= now);
            if due && advance(task, now, &mut out) {
                let mut finished = self.tasks.remove(index);
                out.extend(release_all(&mut finished.held));
            } else {
                index += 1;
            }
        }
        out
    }

    pub fn next_deadline(&self) -> Option<Duration> {
        self.tasks.iter().filter_map(|task| task.due).min()
    }

    fn find(&self, source: u8) -> Option<usize> {
        self.tasks.iter().position(|task| task.source == source)
    }

    fn stop(&mut self, index: usize) -> Vec<SinkEvent> {
        let mut task = self.tasks.remove(index);
        release_all(&mut task.held)
    }
}

fn release_all(held: &mut Vec<SinkEvent>) -> Vec<SinkEvent> {
    held.drain(..).rev().filter_map(|event| event.release_of()).collect()
}

fn advance(task: &mut Task, now: Duration, out: &mut Vec<SinkEvent>) -> bool {
    let Task { held, due, body, .. } = task;
    let Some(scheduled) = *due else { return false };
    match body {
        Body::Hold { repeat } => {
            out.extend(repeat.clone());
            let next = scheduled + TYPEMATIC_INTERVAL;
            *due = Some(if next > now { next } else { now + TYPEMATIC_INTERVAL });
            false
        }
        Body::Macro { steps, mode, position } => {
            let mut at = if now > scheduled + LOOP_GAP { now } else { scheduled };
            loop {
                let Some(step) = steps.get(*position) else {
                    if *mode == RepeatMode::Once {
                        return true;
                    }
                    *position = 0;
                    *due = Some(at + LOOP_GAP);
                    return false;
                };
                *position += 1;
                if let MacroStep::Delay(ms) = step {
                    at += Duration::from_millis(u64::from(*ms));
                    if at > now {
                        *due = Some(at);
                        return false;
                    }
                } else {
                    apply_step(step, held, out);
                }
            }
        }
    }
}

fn apply_step(step: &MacroStep, held: &mut Vec<SinkEvent>, out: &mut Vec<SinkEvent>) {
    match step {
        MacroStep::Down(key) => hold(SinkEvent::key(*key, true), held, out),
        MacroStep::Up(key) => lift(SinkEvent::key(*key, false), held, out),
        MacroStep::Tap(key) => out.extend([SinkEvent::key(*key, true), SinkEvent::key(*key, false)]),
        MacroStep::MouseDown(button) => hold(SinkEvent::Mouse { button: *button, down: true }, held, out),
        MacroStep::MouseUp(button) => lift(SinkEvent::Mouse { button: *button, down: false }, held, out),
        MacroStep::Click(button) => out.extend([
            SinkEvent::Mouse { button: *button, down: true },
            SinkEvent::Mouse { button: *button, down: false },
        ]),
        MacroStep::Wheel(clicks) => out.push(SinkEvent::Wheel { axis: Axis::Vertical, clicks: *clicks }),
        MacroStep::Hwheel(clicks) => out.push(SinkEvent::Wheel { axis: Axis::Horizontal, clicks: *clicks }),
        MacroStep::Text(text) => out.push(SinkEvent::Text(text.clone())),
        MacroStep::Delay(_) => {}
    }
}

fn hold(event: SinkEvent, held: &mut Vec<SinkEvent>, out: &mut Vec<SinkEvent>) {
    if !held.contains(&event) {
        held.push(event.clone());
    }
    out.push(event);
}

fn lift(event: SinkEvent, held: &mut Vec<SinkEvent>, out: &mut Vec<SinkEvent>) {
    held.retain(|down| down.release_of().as_ref() != Some(&event));
    out.push(event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MouseButton;
    use crate::keys::Key;

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn down(name: &str) -> SinkEvent {
        SinkEvent::key(key(name), true)
    }

    fn up(name: &str) -> SinkEvent {
        SinkEvent::key(key(name), false)
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn program(steps: Vec<MacroStep>, mode: RepeatMode) -> Program {
        Program::macro_steps(&steps, mode)
    }

    #[test]
    fn remaps_hold_and_repeat_like_a_keyboard() {
        let mut player = Player::new(true);
        let remap = Program::remap(key("a"), &[key("left_shift")]);
        assert!(player.is_idle());
        assert_eq!(player.press(1, remap, ms(0)), [down("left_shift"), down("a")]);
        assert!(!player.is_idle());
        assert_eq!(player.next_deadline(), Some(ms(500)));
        assert!(player.poll(ms(499)).is_empty());
        assert_eq!(player.poll(ms(500)), [down("a")]);
        assert_eq!(player.poll(ms(533)), [down("a")]);
        assert_eq!(player.release(1, ms(540)), [up("a"), up("left_shift")]);
        assert_eq!(player.next_deadline(), None);
        assert!(player.is_idle());
    }

    #[test]
    fn no_software_repeat_when_the_os_repeats() {
        let mut player = Player::new(false);
        player.press(1, Program::remap(key("a"), &[]), ms(0));
        assert_eq!(player.next_deadline(), None);
        assert!(player.poll(ms(1000)).is_empty());
    }

    #[test]
    fn modifier_remaps_do_not_repeat() {
        let mut player = Player::new(true);
        player.press(1, Program::remap(key("left_ctrl"), &[]), ms(0));
        assert_eq!(player.next_deadline(), None);
    }

    #[test]
    fn once_macros_run_with_delays_and_ignore_repeat_presses() {
        let steps = vec![
            MacroStep::Down(key("left_ctrl")),
            MacroStep::Tap(key("c")),
            MacroStep::Delay(50),
            MacroStep::Up(key("left_ctrl")),
        ];
        let mut player = Player::new(false);
        assert_eq!(
            player.press(7, program(steps.clone(), RepeatMode::Once), ms(0)),
            [down("left_ctrl"), down("c"), up("c")]
        );
        assert_eq!(player.next_deadline(), Some(ms(50)));
        assert!(player.press(7, program(steps, RepeatMode::Once), ms(10)).is_empty());
        assert!(player.release(7, ms(20)).is_empty());
        assert_eq!(player.poll(ms(50)), [up("left_ctrl")]);
        assert!(!player.is_playing(7));
        assert_eq!(player.next_deadline(), None);
    }

    #[test]
    fn once_macros_release_keys_they_left_down() {
        let mut player = Player::new(false);
        assert_eq!(
            player.press(7, program(vec![MacroStep::Down(key("a"))], RepeatMode::Once), ms(0)),
            [down("a"), up("a")]
        );
        assert!(!player.is_playing(7));
    }

    #[test]
    fn while_held_loops_until_release() {
        let steps = vec![MacroStep::Tap(key("x")), MacroStep::Delay(30)];
        let mut player = Player::new(false);
        assert_eq!(player.press(9, program(steps, RepeatMode::WhileHeld), ms(0)), [down("x"), up("x")]);
        assert!(player.poll(ms(30)).is_empty(), "end of the first iteration");
        assert_eq!(player.next_deadline(), Some(ms(40)));
        assert_eq!(player.poll(ms(40)), [down("x"), up("x")]);
        assert!(player.release(9, ms(50)).is_empty());
        assert!(!player.is_playing(9));
    }

    #[test]
    fn toggles_start_and_stop_on_presses() {
        let steps =
            vec![MacroStep::Down(key("a")), MacroStep::Delay(100), MacroStep::Up(key("a")), MacroStep::Delay(100)];
        let mut player = Player::new(false);
        assert_eq!(player.press(9, program(steps.clone(), RepeatMode::Toggle), ms(0)), [down("a")]);
        assert!(player.release(9, ms(10)).is_empty());
        assert!(player.is_playing(9));
        assert_eq!(player.press(9, program(steps, RepeatMode::Toggle), ms(20)), [up("a")]);
        assert!(!player.is_playing(9));
    }

    #[test]
    fn zero_delay_loops_wait_between_iterations() {
        let mut player = Player::new(false);
        assert_eq!(
            player.press(9, program(vec![MacroStep::Tap(key("a"))], RepeatMode::WhileHeld), ms(0)),
            [down("a"), up("a")]
        );
        assert_eq!(player.next_deadline(), Some(LOOP_GAP));
        assert_eq!(player.poll(ms(10)), [down("a"), up("a")]);
    }

    #[test]
    fn a_late_loop_runs_one_iteration_and_resyncs() {
        let mut player = Player::new(false);
        player.press(9, program(vec![MacroStep::Tap(key("a"))], RepeatMode::WhileHeld), ms(0));
        assert_eq!(player.poll(ms(1010)), [down("a"), up("a")]);
        assert_eq!(player.next_deadline(), Some(ms(1010) + LOOP_GAP));
        assert!(player.poll(ms(1010)).is_empty(), "no burst of missed iterations");
    }

    #[test]
    fn a_late_macro_keeps_its_remaining_delays() {
        let steps = vec![
            MacroStep::Tap(key("a")),
            MacroStep::Delay(100),
            MacroStep::Tap(key("b")),
            MacroStep::Delay(100),
            MacroStep::Tap(key("c")),
        ];
        let mut player = Player::new(false);
        assert_eq!(player.press(7, program(steps, RepeatMode::Once), ms(0)), [down("a"), up("a")]);
        assert_eq!(player.poll(ms(1100)), [down("b"), up("b")]);
        assert_eq!(player.next_deadline(), Some(ms(1200)));
        assert!(player.poll(ms(1100)).is_empty());
        assert_eq!(player.poll(ms(1200)), [down("c"), up("c")]);
        assert!(!player.is_playing(7));
    }

    #[test]
    fn cancel_all_releases_everything() {
        let mut player = Player::new(true);
        player.press(1, Program::remap(key("a"), &[key("left_shift")]), ms(0));
        player.press(2, program(vec![MacroStep::Down(key("b")), MacroStep::Delay(100)], RepeatMode::Once), ms(0));
        assert_eq!(player.cancel_all(), [up("a"), up("left_shift"), up("b")]);
        assert_eq!(player.next_deadline(), None);
    }

    #[test]
    fn mouse_buttons_hold_until_release() {
        let mut player = Player::new(true);
        let press = SinkEvent::Mouse { button: MouseButton::Middle, down: true };
        assert_eq!(player.press(3, Program::mouse(MouseButton::Middle), ms(0)), [press]);
        assert_eq!(player.next_deadline(), None);
        assert_eq!(player.release(3, ms(5)), [SinkEvent::Mouse { button: MouseButton::Middle, down: false }]);
    }
}
