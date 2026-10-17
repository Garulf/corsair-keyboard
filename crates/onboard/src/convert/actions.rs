use actions::{
    Action, Assignment, DialConfig, DialMode, DialModeKind, Key, MacroStep, MouseButton, RepeatMode, key_mappings,
};

use super::Report;
use crate::action::DELAY24_MAX;
use crate::usage::{Usage, hid_usage};
use crate::{
    DialModeEntry, KEY_COUNT, MacroAction, MacroEvent, RemapAction, RemapTarget, StoredFile, TextAction, execution,
};

const MAX_REMAP_KEYS: usize = u8::MAX as usize;
const MAX_MACRO_EVENTS: usize = u16::MAX as usize;
const MAX_TEXT_BYTES: usize = u16::MAX as usize;
const LINE_FEED: u8 = 0x0A;
const PRINTABLE_ASCII: std::ops::RangeInclusive<u8> = 0x20..=0x7E;

const NEEDS_THE_PC: &str = "needs the PC, so the key keeps its native output";
const NO_HARDWARE_LAYOUT: &str = "has no hardware layout, so the key keeps its native output";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionFile {
    Remap(RemapAction),
    Macro(MacroAction),
    Text(TextAction),
}

impl ActionFile {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Remap(_) => "remap",
            Self::Macro(_) => "macro",
            Self::Text(_) => "text",
        }
    }

    pub fn stored(&self) -> StoredFile {
        match self {
            Self::Remap(remap) => StoredFile::Remap(remap.clone()),
            Self::Macro(action) => StoredFile::Macro(action.clone()),
            Self::Text(text) => StoredFile::Text(text.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyAction {
    pub key: u8,
    pub action: ActionFile,
    pub execution: u16,
}

enum Outcome {
    Written(ActionFile, u16),
    Suppressed,
}

struct Notes<'a> {
    key: Key,
    report: &'a mut Report,
}

impl Notes<'_> {
    fn skip(&mut self, reason: &str) {
        self.report.skipped.push(format!("{}: {reason}", self.key));
    }

    fn approximate(&mut self, what: &str) {
        self.report.approximated.push(format!("{}: {what}", self.key));
    }
}

pub fn convert_assignments(assignments: &[Assignment], report: &mut Report) -> (Vec<KeyAction>, Vec<u8>) {
    let mut actions = Vec::new();
    let mut kept = Vec::new();
    for assignment in assignments {
        let Some(index) = assignment.key.index() else {
            report.skipped.push(format!("{}: is not a key on the keyboard", assignment.key));
            continue;
        };
        let mut notes = Notes { key: assignment.key, report };
        match convert_action(&assignment.action, &mut notes) {
            Ok(Outcome::Written(action, execution)) => {
                actions.push(KeyAction { key: index, action, execution });
                kept.push(assignment.clone());
            }
            Ok(Outcome::Suppressed) => kept.push(assignment.clone()),
            Err(reason) => notes.skip(&reason),
        }
    }
    (actions, key_mappings(&kept, KEY_COUNT).as_bytes().to_vec())
}

fn convert_action(action: &Action, notes: &mut Notes) -> Result<Outcome, String> {
    match action {
        Action::Remap { key, modifiers } => {
            let target = remap_target(*key, modifiers, notes)?;
            Ok(remap(target))
        }
        Action::Mouse { button } => Ok(remap(RemapTarget::Mouse(mouse_button(*button)))),
        Action::Macro { steps, mode } => convert_macro(steps, *mode, notes),
        Action::Text { text } => convert_text(text, notes),
        Action::Disable => Ok(Outcome::Suppressed),
        Action::Launch { .. }
        | Action::OpenUrl { .. }
        | Action::SwitchProfile { .. }
        | Action::NextProfile
        | Action::PreviousProfile
        | Action::LightingTrigger { .. } => Err(NEEDS_THE_PC.into()),
        Action::BrightnessUp | Action::BrightnessDown | Action::SetBrightness { .. } => Err(NO_HARDWARE_LAYOUT.into()),
    }
}

fn remap(target: RemapTarget) -> Outcome {
    Outcome::Written(ActionFile::Remap(RemapAction::new(target)), execution::REMAP_WHILE_PRESSED)
}

fn remap_target(key: Key, modifiers: &[Key], notes: &mut Notes) -> Result<RemapTarget, String> {
    match hid_usage(key) {
        None => Err(format!("{key} has no hardware usage")),
        Some(Usage::Consumer(usage)) => {
            if !modifiers.is_empty() {
                notes.approximate(&format!("{key} is a media key, so its modifiers are dropped"));
            }
            Ok(RemapTarget::Consumer(usage))
        }
        Some(Usage::Keyboard(usage)) => {
            let mut usages = modifiers
                .iter()
                .map(|modifier| match hid_usage(*modifier) {
                    Some(Usage::Keyboard(usage)) => Ok(usage),
                    _ => Err(format!("modifier {modifier} has no keyboard usage")),
                })
                .collect::<Result<Vec<u8>, String>>()?;
            usages.push(usage);
            if usages.len() > MAX_REMAP_KEYS {
                return Err(format!("the remap lists more than {MAX_REMAP_KEYS} keys"));
            }
            Ok(RemapTarget::Keyboard(usages))
        }
    }
}

fn mouse_button(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 3,
        MouseButton::Back => 4,
        MouseButton::Forward => 5,
    }
}

fn convert_macro(steps: &[MacroStep], mode: RepeatMode, notes: &mut Notes) -> Result<Outcome, String> {
    let mut events = Vec::new();
    for step in steps {
        push_step_events(step, notes, &mut events);
    }
    if events.is_empty() {
        return Err("no macro step has a hardware equivalent".into());
    }
    if events.len() > MAX_MACRO_EVENTS {
        notes.approximate(&format!("the macro is cut to its first {MAX_MACRO_EVENTS} events"));
        events.truncate(MAX_MACRO_EVENTS);
    }
    let execution = match mode {
        RepeatMode::Once => execution::MACRO_ON_PRESS,
        RepeatMode::WhileHeld => {
            notes.approximate("repeat while_held is approximated by the hardware's hold option");
            execution::MACRO_WHILE_PRESSED
        }
        RepeatMode::Toggle => {
            notes.approximate("repeat toggle is approximated by the hardware's toggle option");
            execution::MACRO_TOGGLE
        }
    };
    Ok(Outcome::Written(ActionFile::Macro(MacroAction { events }), execution))
}

fn push_step_events(step: &MacroStep, notes: &mut Notes, events: &mut Vec<MacroEvent>) {
    match step {
        MacroStep::Down(key) => {
            events.extend(keyboard_usage(*key, "down", notes).map(MacroEvent::KeyDown));
        }
        MacroStep::Up(key) => {
            events.extend(keyboard_usage(*key, "up", notes).map(MacroEvent::KeyUp));
        }
        MacroStep::Tap(key) => {
            if let Some(usage) = keyboard_usage(*key, "tap", notes) {
                events.extend([MacroEvent::KeyDown(usage), MacroEvent::KeyUp(usage)]);
            }
        }
        MacroStep::Delay(ms) => {
            if *ms > DELAY24_MAX {
                notes.approximate(&format!("a delay of {ms} ms is cut to {DELAY24_MAX} ms"));
            }
            events.push(MacroEvent::delay(*ms));
        }
        MacroStep::Text(_) => notes.approximate("a macro text step has no hardware event and is dropped"),
        MacroStep::MouseDown(button) => events.push(MacroEvent::MouseDown(mouse_button(*button))),
        MacroStep::MouseUp(button) => events.push(MacroEvent::MouseUp(mouse_button(*button))),
        MacroStep::Click(button) => {
            let button = mouse_button(*button);
            events.extend([MacroEvent::MouseDown(button), MacroEvent::MouseUp(button)]);
        }
        MacroStep::Wheel(amount) => {
            events.push(MacroEvent::VerticalScroll(scroll_amount(*amount, "vertical", notes)));
        }
        MacroStep::Hwheel(amount) => {
            events.push(MacroEvent::HorizontalScroll(scroll_amount(*amount, "horizontal", notes)));
        }
    }
}

fn keyboard_usage(key: Key, step: &str, notes: &mut Notes) -> Option<u8> {
    match hid_usage(key) {
        Some(Usage::Keyboard(usage)) => Some(usage),
        _ => {
            notes.approximate(&format!("macro step {step} {key} has no keyboard usage and is dropped"));
            None
        }
    }
}

fn scroll_amount(amount: i32, direction: &str, notes: &mut Notes) -> i8 {
    let clamped = amount.clamp(i32::from(i8::MIN), i32::from(i8::MAX));
    if clamped != amount {
        notes.approximate(&format!("a {direction} scroll of {amount} is cut to {clamped}"));
    }
    i8::try_from(clamped).expect("clamped into the i8 range")
}

fn convert_text(text: &str, notes: &mut Notes) -> Result<Outcome, String> {
    let mut bytes = Vec::new();
    let mut dropped = 0;
    for character in text.chars() {
        match u8::try_from(character) {
            Ok(byte) if PRINTABLE_ASCII.contains(&byte) || byte == LINE_FEED => bytes.push(byte),
            _ => dropped += 1,
        }
    }
    if bytes.is_empty() {
        return Err("no character of the text has a hardware equivalent".into());
    }
    if dropped > 0 {
        notes.approximate(&format!("{dropped} characters outside printable ASCII are dropped from the text"));
    }
    if bytes.len() > MAX_TEXT_BYTES {
        notes.approximate(&format!("the text is cut to its first {MAX_TEXT_BYTES} bytes"));
        bytes.truncate(MAX_TEXT_BYTES);
    }
    Ok(Outcome::Written(ActionFile::Text(TextAction::new(bytes)), execution::TEXT))
}

pub fn convert_dial(dial: &DialConfig, report: &mut Report) -> Vec<DialModeEntry> {
    let configured_by_the_user = !dial.is_default();
    let mut entries = Vec::new();
    for mode in &dial.modes {
        let described = describe(mode);
        match firmware_type(mode.kind) {
            Ok(kind) => {
                if mode.name.is_some() {
                    report.approximated.push(format!("dial mode {described}: the name has no hardware setting"));
                }
                entries.push(DialModeEntry::new(kind, mode.color()));
            }
            Err(reason) if configured_by_the_user => report.skipped.push(format!("dial mode {described}: {reason}")),
            Err(_) => {}
        }
    }
    entries
}

fn describe(mode: &DialMode) -> String {
    match &mode.name {
        Some(name) => format!("{} \"{name}\"", mode.kind.label()),
        None => mode.kind.label().to_owned(),
    }
}

fn firmware_type(kind: DialModeKind) -> Result<u8, &'static str> {
    match kind {
        DialModeKind::Brightness => Ok(0x01),
        DialModeKind::MediaJogging => Ok(0x03),
        DialModeKind::TrackSelector => Ok(0x08),
        DialModeKind::VerticalScroll => Ok(0x04),
        DialModeKind::HorizontalScroll => Ok(0x05),
        DialModeKind::Zoom => Ok(0x07),
        DialModeKind::SwitchApps => Ok(0x06),
        DialModeKind::MacroRecording => Err("recording macros needs the PC"),
        DialModeKind::Custom => Err("custom dial actions need the PC"),
    }
}

#[cfg(test)]
mod tests {
    use actions::{
        Action, Assignment, DialConfig, DialMode, DialModeKind, Key, MacroStep, MouseButton, PassThrough, RepeatMode,
    };
    use k100::Rgb;

    use super::*;
    use crate::{DialModeEntry, KEY_COUNT, MacroEvent, RemapTarget, execution, fixtures};

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn assign(name: &str, action: Action) -> Assignment {
        Assignment { key: key(name), action, pass_through: vec![] }
    }

    fn run(assignments: &[Assignment]) -> (Vec<KeyAction>, Vec<u8>, Report) {
        let mut report = Report::default();
        let (actions, mappings) = convert_assignments(assignments, &mut report);
        (actions, mappings, report)
    }

    #[test]
    fn a_remap_with_modifiers_matches_the_icue_file() {
        let remap = Action::Remap { key: key("backslash"), modifiers: vec![key("right_alt"), key("right_shift")] };
        let (actions, mappings, report) = run(&[assign("caps_lock", remap)]);
        assert_eq!((actions[0].key, actions[0].execution), (57, execution::REMAP_WHILE_PRESSED));
        let ActionFile::Remap(file) = &actions[0].action else { panic!("expected a remap") };
        assert_eq!(file.encode(), fixtures::file(0x6D62));
        assert_eq!((mappings.len(), mappings[57], mappings[4]), (KEY_COUNT, 0x00, 0x01));
        assert!(report.is_empty());
    }

    #[test]
    fn media_keys_become_consumer_remaps_without_modifiers() {
        let remap = Action::Remap { key: key("volume_up"), modifiers: vec![key("left_ctrl")] };
        let (actions, _, report) = run(&[assign("g2", remap)]);
        assert!(matches!(&actions[0].action, ActionFile::Remap(remap) if remap.target == RemapTarget::Consumer(0xE9)));
        assert!(report.approximated[0].starts_with("g2"), "{report:?}");
    }

    #[test]
    fn mouse_buttons_become_mouse_remaps() {
        let (actions, _, _) = run(&[assign("g4", Action::Mouse { button: MouseButton::Middle })]);
        assert!(matches!(&actions[0].action, ActionFile::Remap(remap) if remap.target == RemapTarget::Mouse(3)));
    }

    #[test]
    fn a_macro_matches_the_format_doc_example() {
        let steps =
            vec![MacroStep::Down(key("a")), MacroStep::Delay(50), MacroStep::Up(key("a")), MacroStep::Tap(key("b"))];
        let (actions, mappings, report) = run(&[assign("g3", Action::Macro { steps, mode: RepeatMode::Once })]);
        use MacroEvent::*;
        let ActionFile::Macro(file) = &actions[0].action else { panic!("expected a macro") };
        assert_eq!(file.events, [KeyDown(4), Delay8(50), KeyUp(4), KeyDown(5), KeyUp(5)]);
        assert_eq!((actions[0].execution, mappings[133]), (execution::MACRO_ON_PRESS, 0x00));
        assert!(report.is_empty());
    }

    #[test]
    fn macro_steps_without_a_hardware_event_are_reported() {
        let steps = vec![
            MacroStep::Tap(key("a")),
            MacroStep::Text("x".into()),
            MacroStep::Down(key("volume_up")),
            MacroStep::Delay(70_000),
            MacroStep::Wheel(-3),
            MacroStep::Hwheel(200),
        ];
        let (actions, _, report) = run(&[assign("g3", Action::Macro { steps, mode: RepeatMode::WhileHeld })]);
        use MacroEvent::*;
        let ActionFile::Macro(file) = &actions[0].action else { panic!("expected a macro") };
        assert_eq!(file.events, [KeyDown(4), KeyUp(4), Delay24(70_000), VerticalScroll(-3), HorizontalScroll(127)]);
        assert_eq!(actions[0].execution, execution::MACRO_WHILE_PRESSED);
        let lines = report.lines().join("\n");
        for expected in ["text", "volume_up", "scroll", "repeat"] {
            assert!(lines.contains(expected), "{expected}: {lines}");
        }
    }

    #[test]
    fn mouse_steps_and_toggle_repeat_convert() {
        let steps = vec![MacroStep::MouseDown(MouseButton::Left), MacroStep::Click(MouseButton::Back)];
        let (actions, _, report) = run(&[assign("g3", Action::Macro { steps, mode: RepeatMode::Toggle })]);
        use MacroEvent::*;
        let ActionFile::Macro(file) = &actions[0].action else { panic!("expected a macro") };
        assert_eq!(file.events, [MouseDown(1), MouseDown(4), MouseUp(4)]);
        assert_eq!(actions[0].execution, execution::MACRO_TOGGLE);
        assert!(report.approximated[0].contains("toggle"), "{report:?}");
    }

    #[test]
    fn a_macro_with_nothing_playable_is_skipped() {
        let steps = vec![MacroStep::Text("x".into())];
        let (actions, mappings, report) = run(&[assign("g3", Action::Macro { steps, mode: RepeatMode::Once })]);
        assert!(actions.is_empty());
        assert_eq!(mappings[133], 0x01);
        assert!(report.skipped[0].starts_with("g3"), "{report:?}");
    }

    #[test]
    fn text_keeps_printable_ascii_and_newlines() {
        let (actions, _, report) = run(&[assign("g1", Action::Text { text: "h\u{e9}!\n".into() })]);
        let ActionFile::Text(file) = &actions[0].action else { panic!("expected text") };
        assert_eq!((file.text.as_slice(), actions[0].execution), (&b"h!\n"[..], execution::TEXT));
        assert!(report.approximated[0].starts_with("g1"), "{report:?}");
    }

    #[test]
    fn text_without_a_hardware_character_is_skipped() {
        let (actions, mappings, report) = run(&[assign("g1", Action::Text { text: "\u{e9}".into() })]);
        assert!(actions.is_empty());
        assert_eq!(mappings[131], 0x01);
        assert!(report.skipped[0].starts_with("g1"), "{report:?}");
    }

    #[test]
    fn text_longer_than_a_file_holds_is_cut_and_reported() {
        let text = "a".repeat(70_000);
        let (actions, _, report) = run(&[assign("g1", Action::Text { text })]);
        let ActionFile::Text(file) = &actions[0].action else { panic!("expected text") };
        assert_eq!(file.text.len(), 65_535);
        assert!(report.approximated[0].contains("65535"), "{report:?}");
    }

    #[test]
    fn disable_suppresses_the_key_without_an_action_file() {
        let (actions, mappings, _) = run(&[assign("g6", Action::Disable)]);
        assert!(actions.is_empty());
        assert_eq!(mappings[136], 0x00);
    }

    #[test]
    fn actions_that_need_the_pc_are_skipped_and_keep_the_key() {
        let launch = Action::Launch { program: "calc".into(), args: vec![] };
        let (actions, mappings, report) = run(&[assign("g5", launch), assign("g1", Action::NextProfile)]);
        assert!(actions.is_empty());
        assert_eq!((mappings[135], mappings[131]), (0x01, 0x01));
        assert_eq!(report.skipped.len(), 2, "{report:?}");
    }

    #[test]
    fn brightness_actions_are_skipped_as_having_no_hardware_layout() {
        let (actions, _, report) = run(&[assign("g1", Action::BrightnessUp)]);
        assert!(actions.is_empty());
        assert!(report.skipped[0].contains("hardware"), "{report:?}");
    }

    #[test]
    fn a_remap_to_a_key_without_a_usage_is_skipped() {
        let (actions, _, report) = run(&[assign("g1", Action::Remap { key: key("g2"), modifiers: vec![] })]);
        assert!(actions.is_empty());
        assert!(report.skipped[0].starts_with("g1"), "{report:?}");
    }

    #[test]
    fn pass_through_keeps_the_original_key_with_that_modifier() {
        let mut caps = assign("caps_lock", Action::Remap { key: key("escape"), modifiers: vec![] });
        caps.pass_through = vec![PassThrough::Shift];
        let (_, mappings, _) = run(&[caps]);
        assert_eq!(mappings[57], 0x20);
    }

    #[test]
    fn native_dial_modes_are_written_with_their_ring_colours() {
        let mut report = Report::default();
        let modes = convert_dial(&DialConfig::default(), &mut report);
        assert_eq!(modes.iter().map(|mode| mode.kind).collect::<Vec<_>>(), [0x01, 0x03, 0x08]);
        assert_eq!(modes[0], DialModeEntry::new(0x01, Rgb::new(0x00, 0xBF, 0xFF)));
    }

    #[test]
    fn a_profile_that_never_configured_the_dial_reports_nothing_about_it() {
        let mut report = Report::default();
        convert_dial(&DialConfig::default(), &mut report);
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn a_configured_macro_recording_mode_is_reported_as_skipped() {
        let modes = vec![DialMode::builtin(DialModeKind::Brightness), DialMode::builtin(DialModeKind::MacroRecording)];
        let mut report = Report::default();
        let written = convert_dial(&DialConfig { modes }, &mut report);
        assert_eq!(written.len(), 1);
        assert_eq!(report.skipped, ["dial mode macro_recording: recording macros needs the PC"]);
    }

    #[test]
    fn every_native_dial_kind_has_its_firmware_type_and_custom_is_skipped() {
        use DialModeKind::*;
        let kinds = [Brightness, MediaJogging, TrackSelector, VerticalScroll, HorizontalScroll, Zoom, SwitchApps];
        let mut modes: Vec<DialMode> = kinds.into_iter().map(DialMode::builtin).collect();
        modes.push(DialMode { clockwise: Some(Action::Disable), ..DialMode::builtin(Custom) });
        let mut report = Report::default();
        let written = convert_dial(&DialConfig { modes }, &mut report);
        assert_eq!(
            written.iter().map(|mode| mode.kind).collect::<Vec<_>>(),
            [0x01, 0x03, 0x08, 0x04, 0x05, 0x07, 0x06]
        );
        assert!(report.skipped[0].contains("custom"), "{report:?}");
    }

    #[test]
    fn a_dial_mode_name_has_no_hardware_setting() {
        let named = DialMode { name: Some("Lights".into()), ..DialMode::builtin(DialModeKind::Brightness) };
        let mut report = Report::default();
        let written = convert_dial(&DialConfig { modes: vec![named] }, &mut report);
        assert_eq!(written.len(), 1);
        assert!(report.approximated[0].contains("name"), "{report:?}");
    }
}
