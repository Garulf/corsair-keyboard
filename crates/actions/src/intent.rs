use crate::config::{Action, MacroStep};
use crate::keys::Key;
use crate::program::Program;
use crate::recorder::RecorderState;
use crate::sink::SinkEvent;
use crate::worker::OutputCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileTarget {
    Next,
    Previous,
    Named(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrightnessChange {
    Up,
    Down,
    Set(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Output(OutputCommand),
    SwitchProfile(ProfileTarget),
    Brightness(BrightnessChange),
    LightingTrigger { layer: String, on: bool, duration_ms: Option<u32> },
    DialMode { index: usize },
    Recording(RecorderState),
    SaveMacro { key: Key, steps: Vec<MacroStep> },
}

pub fn action_intents(action: &Action, source: u8, pressed: bool) -> Vec<Intent> {
    let output = |command| vec![Intent::Output(command)];
    match (action, pressed) {
        (Action::Remap { key, modifiers }, true) => {
            output(OutputCommand::Press { source, program: Program::remap(*key, modifiers) })
        }
        (Action::Mouse { button }, true) => output(OutputCommand::Press { source, program: Program::mouse(*button) }),
        (Action::Macro { steps, mode }, true) => {
            output(OutputCommand::Press { source, program: Program::macro_steps(steps, *mode) })
        }
        (Action::Remap { .. } | Action::Mouse { .. } | Action::Macro { .. }, false) => {
            output(OutputCommand::Release { source })
        }
        (Action::Text { text }, true) => output(OutputCommand::Emit(vec![SinkEvent::Text(text.clone())])),
        (Action::Launch { program, args }, true) => {
            output(OutputCommand::Launch { program: program.clone(), args: args.clone() })
        }
        (Action::OpenUrl { url }, true) => output(OutputCommand::OpenUrl { url: url.clone() }),
        (Action::SwitchProfile { name }, true) => vec![Intent::SwitchProfile(ProfileTarget::Named(name.clone()))],
        (Action::NextProfile, true) => vec![Intent::SwitchProfile(ProfileTarget::Next)],
        (Action::PreviousProfile, true) => vec![Intent::SwitchProfile(ProfileTarget::Previous)],
        (Action::LightingTrigger { layer, duration_ms }, true) => {
            vec![Intent::LightingTrigger { layer: layer.clone(), on: true, duration_ms: *duration_ms }]
        }
        (Action::LightingTrigger { layer, duration_ms: None }, false) => {
            vec![Intent::LightingTrigger { layer: layer.clone(), on: false, duration_ms: None }]
        }
        (Action::BrightnessUp, true) => vec![Intent::Brightness(BrightnessChange::Up)],
        (Action::BrightnessDown, true) => vec![Intent::Brightness(BrightnessChange::Down)],
        (Action::SetBrightness { percent }, true) => vec![Intent::Brightness(BrightnessChange::Set(*percent))],
        _ => Vec::new(),
    }
}

pub fn one_shot(action: &Action, source: u8) -> Vec<Intent> {
    let mut intents = action_intents(action, source, true);
    intents.extend(action_intents(action, source, false));
    intents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Key;

    #[test]
    fn held_actions_press_and_release() {
        let remap = Action::Remap { key: Key::named("escape").unwrap(), modifiers: vec![] };
        let program = Program::remap(Key::named("escape").unwrap(), &[]);
        assert_eq!(action_intents(&remap, 57, true), [Intent::Output(OutputCommand::Press { source: 57, program })]);
        assert_eq!(action_intents(&remap, 57, false), [Intent::Output(OutputCommand::Release { source: 57 })]);
    }

    #[test]
    fn instant_actions_fire_on_press_only() {
        let text = Action::Text { text: "hi".into() };
        assert_eq!(
            action_intents(&text, 131, true),
            [Intent::Output(OutputCommand::Emit(vec![SinkEvent::Text("hi".into())]))]
        );
        assert!(action_intents(&text, 131, false).is_empty());
        assert_eq!(action_intents(&Action::NextProfile, 131, true), [Intent::SwitchProfile(ProfileTarget::Next)]);
        assert_eq!(
            action_intents(&Action::SetBrightness { percent: 40 }, 131, true),
            [Intent::Brightness(BrightnessChange::Set(40))]
        );
        assert!(action_intents(&Action::Disable, 131, true).is_empty());
    }

    #[test]
    fn lighting_triggers_follow_the_key_unless_timed() {
        let held = Action::LightingTrigger { layer: "alert".into(), duration_ms: None };
        let off = Intent::LightingTrigger { layer: "alert".into(), on: false, duration_ms: None };
        assert_eq!(action_intents(&held, 131, false), [off]);
        let timed = Action::LightingTrigger { layer: "alert".into(), duration_ms: Some(500) };
        assert!(action_intents(&timed, 131, false).is_empty());
        assert_eq!(one_shot(&timed, 137).len(), 1);
    }
}
