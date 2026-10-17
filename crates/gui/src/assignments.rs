use std::collections::BTreeSet;

use actions::{Action, Assignment, Key, PassThrough, RecorderState, RepeatMode};
use eframe::egui;
use ipc::{DeviceState, Request};

use crate::actions_editor::{Choices, action_editor};
use crate::model::Model;

pub fn assignability(led: u8) -> Result<Key, String> {
    let Some(key) = Key::from_index(led) else {
        return Err("This LED is not a key; pick a key on the keyboard.".into());
    };
    match led {
        k100::led::DIAL_KEY => Err("The dial button cannot be reassigned; use the dial pane for its modes.".into()),
        k100::led::WIN_LOCK => Err("The WinLock key cannot be reassigned.".into()),
        _ if !key.is_on_k100() => Err("The K100 does not have this key.".into()),
        _ => Ok(key),
    }
}

pub fn assignment_index(assignments: &[Assignment], key: Key) -> Option<usize> {
    assignments.iter().position(|assignment| assignment.key == key)
}

pub fn assign(assignments: &mut Vec<Assignment>, key: Key, action: Action) {
    match assignment_index(assignments, key) {
        Some(index) => assignments[index].action = action,
        None => assignments.push(Assignment { key, action, pass_through: Vec::new() }),
    }
}

pub fn unassign(assignments: &mut Vec<Assignment>, key: Key) -> bool {
    let before = assignments.len();
    assignments.retain(|assignment| assignment.key != key);
    assignments.len() != before
}

pub fn action_summary(action: &Action) -> String {
    match action {
        Action::Remap { key, modifiers } => {
            let mut names: Vec<&str> = modifiers.iter().map(|modifier| modifier.name()).collect();
            names.push(key.name());
            format!("Remap to {}", names.join("+"))
        }
        Action::Macro { steps, mode } => {
            let repeat = match mode {
                RepeatMode::Once => "",
                RepeatMode::WhileHeld => ", while held",
                RepeatMode::Toggle => ", toggle",
            };
            let count = steps.len();
            format!("Macro, {count} step{}{repeat}", if count == 1 { "" } else { "s" })
        }
        Action::Text { text } => format!("Type {text:?}"),
        Action::Launch { program, .. } => format!("Launch {program}"),
        Action::OpenUrl { url } => format!("Open {url}"),
        Action::Mouse { button } => format!("Mouse {button:?}").to_lowercase(),
        Action::Disable => "Disabled".into(),
        Action::SwitchProfile { name } => format!("Switch to {name}"),
        Action::NextProfile => "Next profile".into(),
        Action::PreviousProfile => "Previous profile".into(),
        Action::LightingTrigger { layer, .. } => format!("Light up {layer}"),
        Action::BrightnessUp => "Brightness up".into(),
        Action::BrightnessDown => "Brightness down".into(),
        Action::SetBrightness { percent } => format!("Set brightness to {percent}%"),
    }
}

pub fn recording_text(state: RecorderState) -> String {
    match state {
        RecorderState::Idle => "Not recording".into(),
        RecorderState::AwaitingKey => "Press the G key to record into".into(),
        RecorderState::Recording { key } => {
            let name = Key::from_index(key).map_or(key.to_string(), |key| key.name().to_owned());
            format!("Recording into {name}: press the same G key again to finish")
        }
    }
}

pub fn recording_lock(state: RecorderState, key: Key) -> Option<&'static str> {
    let index = key.index()?;
    match state {
        RecorderState::Idle => None,
        RecorderState::AwaitingKey if (k100::led::G1..=k100::led::G6).contains(&index) => {
            Some("The recorder is waiting for a G key; G keys can be edited once it finishes.")
        }
        RecorderState::AwaitingKey => None,
        RecorderState::Recording { key } if key == index => {
            Some("A macro is being recorded into this key; it can be edited once the recording ends.")
        }
        RecorderState::Recording { .. } => None,
    }
}

pub fn recording_blocker(model: &Model) -> Option<String> {
    let connected = model.state.as_ref().is_some_and(|state| matches!(state.device, DeviceState::Connected { .. }));
    if !connected {
        return Some("No keyboard is connected.".into());
    }
    let draft = model.draft.as_ref()?;
    if draft.dirty() {
        return Some("Apply or revert your edits first: recorded macros are saved into the active profile.".into());
    }
    if draft.profile.name != model.active_profile() {
        return Some("Switch to this profile first: macros are recorded into the active profile.".into());
    }
    None
}

fn single(selection: &BTreeSet<u8>) -> Option<u8> {
    let mut leds = selection.iter();
    match (leds.next(), leds.next()) {
        (Some(led), None) => Some(*led),
        _ => None,
    }
}

pub fn keys(ui: &mut egui::Ui, model: &mut Model) {
    crate::canvas::pane(ui, model);
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |columns| {
            assignment_list(&mut columns[0], model);
            key_editor(&mut columns[1], model);
        });
    });
}

fn assignment_list(ui: &mut egui::Ui, model: &mut Model) {
    ui.strong("Assigned keys");
    let Some(draft) = model.draft.as_ref() else { return };
    if draft.profile.assignments.is_empty() {
        ui.label("No keys are assigned in this profile.");
    }
    let mut picked = None;
    for assignment in &draft.profile.assignments {
        let text = format!("{}: {}", assignment.key, action_summary(&assignment.action));
        let selected = assignment.key.index().is_some_and(|index| model.selection.contains(&index));
        if ui.selectable_label(selected, text).clicked() {
            picked = assignment.key.index();
        }
    }
    if let Some(index) = picked {
        model.selection = BTreeSet::from([index]);
    }
}

fn key_editor(ui: &mut egui::Ui, model: &mut Model) {
    ui.strong("Selected key");
    let Some(led) = single(&model.selection) else {
        ui.label("Click one key on the keyboard to assign an action to it.");
        recorder(ui, model);
        return;
    };
    match assignability(led) {
        Err(reason) => {
            ui.label(reason);
        }
        Ok(key) => locked_assignment_editor(ui, model, key),
    }
    recorder(ui, model);
}

fn locked_assignment_editor(ui: &mut egui::Ui, model: &mut Model, key: Key) {
    let state = model.state.as_ref().map_or(RecorderState::Idle, |state| state.recording);
    let lock = recording_lock(state, key);
    if let Some(reason) = lock {
        ui.weak(reason);
    }
    ui.add_enabled_ui(lock.is_none(), |ui| assignment_editor(ui, model, key));
}

fn assignment_editor(ui: &mut egui::Ui, model: &mut Model, key: Key) {
    let profiles = model.state.as_ref().map(|state| state.profiles.clone()).unwrap_or_default();
    let Some(draft) = model.draft.as_mut() else { return };
    let choices = Choices { layers: draft.profile.layers.iter().map(|layer| layer.name.clone()).collect(), profiles };
    let Some(index) = assignment_index(&draft.profile.assignments, key) else {
        ui.label(format!("{key} is not assigned."));
        if ui.button("Assign an action").clicked() {
            assign(&mut draft.profile.assignments, key, Action::Disable);
        }
        return;
    };
    let mut clear = false;
    let assignment = &mut draft.profile.assignments[index];
    ui.label(format!("{key} does:"));
    action_editor(ui, key.name(), &mut assignment.action, &choices);
    ui.separator();
    for (pass, label) in
        [(PassThrough::Fn, "Fn"), (PassThrough::Ctrl, "Ctrl"), (PassThrough::Alt, "Alt"), (PassThrough::Shift, "Shift")]
    {
        let mut on = assignment.pass_through.contains(&pass);
        if ui.checkbox(&mut on, format!("Still types normally while {label} is held")).changed() {
            assignment.pass_through.retain(|existing| *existing != pass);
            if on {
                assignment.pass_through.push(pass);
            }
        }
    }
    if ui.button("Clear assignment").clicked() {
        clear = true;
    }
    if clear {
        unassign(&mut draft.profile.assignments, key);
    }
}

fn recorder(ui: &mut egui::Ui, model: &mut Model) {
    ui.separator();
    ui.strong("Macro recorder");
    let state = model.state.as_ref().map_or(RecorderState::Idle, |state| state.recording);
    match state {
        RecorderState::Idle => {
            let blocker = recording_blocker(model);
            if ui.add_enabled(blocker.is_none(), egui::Button::new("Record a macro")).clicked() {
                model.send(Request::StartRecording { key: None });
            }
            match blocker {
                Some(reason) => ui.weak(reason),
                None => ui.weak("Press a G key, type the keys, then press the same G key again. The macro is saved into the active profile."),
            };
        }
        RecorderState::AwaitingKey | RecorderState::Recording { .. } => {
            ui.label(recording_text(state));
            if ui.button("Cancel recording").clicked() {
                model.send(Request::CancelRecording);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use actions::MacroStep;
    use egui_kittest::kittest::{NodeT, Queryable};
    use ipc::DeviceState;

    use super::*;
    use crate::testing::{harness, online, sent};

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn assigned(key_name: &str, action: Action) -> Assignment {
        Assignment { key: key(key_name), action, pass_through: vec![] }
    }

    fn keys_model(selected: u8) -> Model {
        let mut model = online("p", &["p"]);
        model.selection.insert(selected);
        model
    }

    fn assignments(model: &Model) -> &Vec<Assignment> {
        &model.draft.as_ref().unwrap().profile.assignments
    }

    #[test]
    fn protected_and_missing_keys_explain_themselves() {
        assert_eq!(assignability(131).unwrap(), key("g1"));
        assert!(assignability(137).unwrap_err().contains("dial"));
        assert!(assignability(114).unwrap_err().contains("WinLock"));
        assert!(assignability(127).unwrap_err().contains("K100"));
        assert!(assignability(113).unwrap_err().contains("K100"));
        assert!(assignability(140).unwrap_err().contains("not a key"));
    }

    #[test]
    fn assigning_replaces_and_clearing_removes() {
        let mut list = vec![assigned("g1", Action::Disable)];
        assign(&mut list, key("g1"), Action::NextProfile);
        assign(&mut list, key("g2"), Action::Disable);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].action, Action::NextProfile);
        assert_eq!(assignment_index(&list, key("g2")), Some(1));
        assert!(unassign(&mut list, key("g1")));
        assert!(!unassign(&mut list, key("g1")));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn summaries_are_short_and_specific() {
        let steps = vec![MacroStep::Tap(key("a")), MacroStep::Delay(5)];
        assert_eq!(action_summary(&Action::Macro { steps, mode: RepeatMode::Toggle }), "Macro, 2 steps, toggle");
        assert_eq!(
            action_summary(&Action::Remap { key: key("escape"), modifiers: vec![key("left_ctrl")] }),
            "Remap to left_ctrl+escape"
        );
        assert_eq!(action_summary(&Action::Text { text: "hello".into() }), "Type \"hello\"");
        assert_eq!(action_summary(&Action::Disable), "Disabled");
        assert_eq!(action_summary(&Action::SetBrightness { percent: 40 }), "Set brightness to 40%");
    }

    #[test]
    fn recording_text_names_the_target_key() {
        assert_eq!(recording_text(RecorderState::Idle), "Not recording");
        assert!(recording_text(RecorderState::AwaitingKey).contains("G key"));
        assert!(recording_text(RecorderState::Recording { key: 131 }).contains("g1"));
    }

    #[test]
    fn recording_is_blocked_when_the_macro_could_be_lost_or_misplaced() {
        let mut model = online("p", &["p"]);
        assert_eq!(recording_blocker(&model), None);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        assert!(recording_blocker(&model).unwrap().contains("Apply or revert"));
        model.discard_draft();
        model.draft = Some(crate::model::Draft::new(crate::testing::profile("other")));
        assert!(recording_blocker(&model).unwrap().contains("active profile"));
        model.draft = Some(crate::model::Draft::new(crate::testing::profile("p")));
        model.state.as_mut().unwrap().device = DeviceState::Disconnected;
        assert!(recording_blocker(&model).unwrap().contains("No keyboard"));
    }

    #[test]
    fn only_the_key_being_recorded_into_is_locked() {
        assert_eq!(recording_lock(RecorderState::Idle, key("g2")), None);
        assert!(recording_lock(RecorderState::Recording { key: 132 }, key("g2")).is_some());
        assert_eq!(recording_lock(RecorderState::Recording { key: 132 }, key("g1")), None);
        assert!(recording_lock(RecorderState::AwaitingKey, key("g1")).is_some(), "any G key may become the target");
        assert_eq!(recording_lock(RecorderState::AwaitingKey, key("a")), None);
    }

    #[test]
    fn an_unassigned_key_can_be_given_an_action() {
        let mut harness = harness(keys_model(131), keys);
        harness.run();
        harness.get_by_label("g1 is not assigned.");
        harness.get_by_label("Assign an action").click();
        harness.run();
        assert_eq!(assignments(harness.state()), &[assigned("g1", Action::Disable)]);
    }

    #[test]
    fn an_assigned_key_shows_its_editor_and_can_be_cleared() {
        let mut model = keys_model(131);
        model.draft.as_mut().unwrap().profile.assignments = vec![assigned("g1", Action::NextProfile)];
        let mut harness = harness(model, keys);
        harness.run();
        harness.get_by_label("Clear assignment").click();
        harness.run();
        assert!(assignments(harness.state()).is_empty());
    }

    #[test]
    fn pass_through_modifiers_toggle() {
        let mut model = keys_model(57);
        model.draft.as_mut().unwrap().profile.assignments =
            vec![assigned("caps_lock", Action::Remap { key: key("escape"), modifiers: vec![] })];
        let mut harness = harness(model, keys);
        harness.run();
        harness.get_by_label("Still types normally while Shift is held").click();
        harness.run();
        assert_eq!(assignments(harness.state())[0].pass_through, [PassThrough::Shift]);
        harness.get_by_label("Still types normally while Shift is held").click();
        harness.run();
        assert!(assignments(harness.state())[0].pass_through.is_empty());
    }

    #[test]
    fn the_dial_button_cannot_be_assigned() {
        let mut harness = harness(keys_model(137), keys);
        harness.run();
        harness.get_by_label_contains("cannot be reassigned");
        assert!(harness.query_by_label("Assign an action").is_none());
    }

    #[test]
    fn the_recorder_starts_and_cancels_through_the_daemon() {
        let mut harness = harness(keys_model(131), keys);
        harness.run();
        harness.get_by_label("Record a macro").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [ipc::Request::StartRecording { key: None }]);
        harness.state_mut().state.as_mut().unwrap().recording = RecorderState::AwaitingKey;
        harness.run();
        harness.get_by_label("Press the G key to record into");
        harness.get_by_label("Cancel recording").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [ipc::Request::CancelRecording]);
    }

    #[test]
    fn the_key_being_recorded_into_cannot_be_edited_until_the_recording_ends() {
        let mut model = keys_model(132);
        model.draft.as_mut().unwrap().profile.assignments =
            vec![assigned("g1", Action::NextProfile), assigned("g2", Action::NextProfile)];
        model.state.as_mut().unwrap().recording = RecorderState::Recording { key: 132 };
        let mut harness = harness(model, keys);
        harness.run();
        assert!(harness.get_by_label("Clear assignment").accesskit_node().is_disabled());
        harness.get_by_label_contains("being recorded");
        harness.state_mut().selection = BTreeSet::from([131]);
        harness.run();
        assert!(!harness.get_by_label("Clear assignment").accesskit_node().is_disabled());
        harness.state_mut().selection = BTreeSet::from([132]);
        harness.state_mut().state.as_mut().unwrap().recording = RecorderState::Idle;
        harness.run();
        assert!(!harness.get_by_label("Clear assignment").accesskit_node().is_disabled());
    }

    #[test]
    fn an_unassigned_key_being_recorded_into_cannot_be_given_an_action() {
        let mut model = keys_model(132);
        model.state.as_mut().unwrap().recording = RecorderState::Recording { key: 132 };
        let mut harness = harness(model, keys);
        harness.run();
        harness.get_by_label("Assign an action").click();
        harness.run();
        assert!(assignments(harness.state()).is_empty());
    }

    #[test]
    fn the_record_button_is_disabled_with_a_reason_while_edits_are_unsaved() {
        let mut model = keys_model(131);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        let mut harness = harness(model, keys);
        harness.run();
        assert!(harness.get_by_label("Record a macro").accesskit_node().is_disabled());
        harness.get_by_label_contains("Apply or revert");
    }

    #[test]
    fn clicking_an_entry_in_the_list_selects_its_key() {
        let mut model = keys_model(131);
        model.draft.as_mut().unwrap().profile.assignments = vec![assigned("g2", Action::NextProfile)];
        let mut harness = harness(model, keys);
        harness.run();
        harness.get_by_label("g2: Next profile").click();
        harness.run();
        assert_eq!(harness.state().selection.iter().copied().collect::<Vec<_>>(), [132]);
        harness.get_by_label("Clear assignment");
    }
}
