use actions::{Action, Key, MacroStep, MouseButton, RepeatMode};
use eframe::egui;

use crate::model::move_item;

const MAX_DELAY_MS: u32 = 600_000;

pub struct Choices {
    pub layers: Vec<String>,
    pub profiles: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Remap,
    Macro,
    Text,
    Launch,
    OpenUrl,
    Mouse,
    Disable,
    SwitchProfile,
    NextProfile,
    PreviousProfile,
    LightingTrigger,
    BrightnessUp,
    BrightnessDown,
    SetBrightness,
}

fn key(name: &str) -> Key {
    Key::named(name).expect("built-in key names are in the key table")
}

impl ActionKind {
    pub const ALL: [ActionKind; 14] = [
        Self::Remap,
        Self::Macro,
        Self::Text,
        Self::Launch,
        Self::OpenUrl,
        Self::Mouse,
        Self::Disable,
        Self::SwitchProfile,
        Self::NextProfile,
        Self::PreviousProfile,
        Self::LightingTrigger,
        Self::BrightnessUp,
        Self::BrightnessDown,
        Self::SetBrightness,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Remap => "Remap to a key",
            Self::Macro => "Macro",
            Self::Text => "Type text",
            Self::Launch => "Launch a program",
            Self::OpenUrl => "Open a web page",
            Self::Mouse => "Mouse button",
            Self::Disable => "Disable the key",
            Self::SwitchProfile => "Switch to a profile",
            Self::NextProfile => "Next profile",
            Self::PreviousProfile => "Previous profile",
            Self::LightingTrigger => "Light up a layer",
            Self::BrightnessUp => "Brightness up",
            Self::BrightnessDown => "Brightness down",
            Self::SetBrightness => "Set brightness",
        }
    }

    pub fn of(action: &Action) -> Self {
        match action {
            Action::Remap { .. } => Self::Remap,
            Action::Macro { .. } => Self::Macro,
            Action::Text { .. } => Self::Text,
            Action::Launch { .. } => Self::Launch,
            Action::OpenUrl { .. } => Self::OpenUrl,
            Action::Mouse { .. } => Self::Mouse,
            Action::Disable => Self::Disable,
            Action::SwitchProfile { .. } => Self::SwitchProfile,
            Action::NextProfile => Self::NextProfile,
            Action::PreviousProfile => Self::PreviousProfile,
            Action::LightingTrigger { .. } => Self::LightingTrigger,
            Action::BrightnessUp => Self::BrightnessUp,
            Action::BrightnessDown => Self::BrightnessDown,
            Action::SetBrightness { .. } => Self::SetBrightness,
        }
    }

    pub fn default_action(self, choices: &Choices) -> Action {
        match self {
            Self::Remap => Action::Remap { key: key("escape"), modifiers: Vec::new() },
            Self::Macro => Action::Macro { steps: vec![MacroStep::Tap(key("a"))], mode: RepeatMode::Once },
            Self::Text => Action::Text { text: String::new() },
            Self::Launch => Action::Launch { program: String::new(), args: Vec::new() },
            Self::OpenUrl => Action::OpenUrl { url: "https://".into() },
            Self::Mouse => Action::Mouse { button: MouseButton::Left },
            Self::Disable => Action::Disable,
            Self::SwitchProfile => {
                Action::SwitchProfile { name: choices.profiles.first().cloned().unwrap_or_default() }
            }
            Self::NextProfile => Action::NextProfile,
            Self::PreviousProfile => Action::PreviousProfile,
            Self::LightingTrigger => Action::LightingTrigger {
                layer: choices.layers.first().cloned().unwrap_or_default(),
                duration_ms: None,
            },
            Self::BrightnessUp => Action::BrightnessUp,
            Self::BrightnessDown => Action::BrightnessDown,
            Self::SetBrightness => Action::SetBrightness { percent: 50 },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Tap,
    Down,
    Up,
    Delay,
    Text,
    Click,
    MouseDown,
    MouseUp,
    Wheel,
    Hwheel,
}

impl StepKind {
    pub const ALL: [StepKind; 10] = [
        Self::Tap,
        Self::Down,
        Self::Up,
        Self::Delay,
        Self::Text,
        Self::Click,
        Self::MouseDown,
        Self::MouseUp,
        Self::Wheel,
        Self::Hwheel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tap => "Tap key",
            Self::Down => "Hold key down",
            Self::Up => "Release key",
            Self::Delay => "Wait",
            Self::Text => "Type text",
            Self::Click => "Click",
            Self::MouseDown => "Hold mouse button",
            Self::MouseUp => "Release mouse button",
            Self::Wheel => "Scroll",
            Self::Hwheel => "Scroll sideways",
        }
    }

    pub fn of(step: &MacroStep) -> Self {
        match step {
            MacroStep::Tap(_) => Self::Tap,
            MacroStep::Down(_) => Self::Down,
            MacroStep::Up(_) => Self::Up,
            MacroStep::Delay(_) => Self::Delay,
            MacroStep::Text(_) => Self::Text,
            MacroStep::Click(_) => Self::Click,
            MacroStep::MouseDown(_) => Self::MouseDown,
            MacroStep::MouseUp(_) => Self::MouseUp,
            MacroStep::Wheel(_) => Self::Wheel,
            MacroStep::Hwheel(_) => Self::Hwheel,
        }
    }

    pub fn default_step(self) -> MacroStep {
        match self {
            Self::Tap => MacroStep::Tap(key("a")),
            Self::Down => MacroStep::Down(key("left_ctrl")),
            Self::Up => MacroStep::Up(key("left_ctrl")),
            Self::Delay => MacroStep::Delay(50),
            Self::Text => MacroStep::Text(String::new()),
            Self::Click => MacroStep::Click(MouseButton::Left),
            Self::MouseDown => MacroStep::MouseDown(MouseButton::Left),
            Self::MouseUp => MacroStep::MouseUp(MouseButton::Left),
            Self::Wheel => MacroStep::Wheel(-1),
            Self::Hwheel => MacroStep::Hwheel(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOp {
    MoveUp,
    MoveDown,
    Delete,
}

pub fn apply_step_op(steps: &mut Vec<MacroStep>, index: usize, op: StepOp) {
    match op {
        StepOp::Delete if index < steps.len() => {
            steps.remove(index);
        }
        StepOp::Delete => {}
        StepOp::MoveUp => {
            move_item(steps, index, false);
        }
        StepOp::MoveDown => {
            move_item(steps, index, true);
        }
    }
}

pub fn sendable_keys() -> Vec<Key> {
    actions::all().iter().filter(|def| def.output.is_some()).filter_map(|def| Key::named(def.name)).collect()
}

pub fn key_picker(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, key: &mut Key) {
    egui::ComboBox::from_id_salt(id).selected_text(key.name()).width(150.0).show_ui(ui, |ui| {
        for choice in sendable_keys() {
            ui.selectable_value(key, choice, choice.name());
        }
    });
}

fn mouse_label(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "Left button",
        MouseButton::Right => "Right button",
        MouseButton::Middle => "Middle button",
        MouseButton::Back => "Back button",
        MouseButton::Forward => "Forward button",
    }
}

fn mouse_picker(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, button: &mut MouseButton) {
    egui::ComboBox::from_id_salt(id).selected_text(mouse_label(*button)).show_ui(ui, |ui| {
        for choice in
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward]
        {
            ui.selectable_value(button, choice, mouse_label(choice));
        }
    });
}

fn split_arguments(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

fn arguments_field(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, args: &mut Vec<String>) {
    let slot = ui.scope_id().with(id);
    let mut text: String = ui
        .data_mut(|data| data.get_temp::<String>(slot))
        .filter(|typed| split_arguments(typed) == *args)
        .unwrap_or_else(|| args.join(" "));
    ui.horizontal(|ui| {
        ui.label("Arguments");
        if ui.text_edit_singleline(&mut text).changed() {
            *args = split_arguments(&text);
        }
    });
    ui.data_mut(|data| data.insert_temp(slot, text));
}

pub fn action_editor(ui: &mut egui::Ui, id: &str, action: &mut Action, choices: &Choices) {
    let mut kind = ActionKind::of(action);
    egui::ComboBox::from_id_salt((id, "kind")).selected_text(kind.label()).width(200.0).show_ui(ui, |ui| {
        for choice in ActionKind::ALL {
            ui.selectable_value(&mut kind, choice, choice.label());
        }
    });
    if kind != ActionKind::of(action) {
        *action = kind.default_action(choices);
    }
    match action {
        Action::Remap { key, modifiers } => {
            ui.horizontal(|ui| {
                ui.label("Key");
                key_picker(ui, (id, "remap"), key);
            });
            ui.horizontal(|ui| {
                for (name, label) in
                    [("left_ctrl", "Ctrl"), ("left_shift", "Shift"), ("left_alt", "Alt"), ("left_meta", "Win")]
                {
                    let modifier = self::key(name);
                    let mut on = modifiers.contains(&modifier);
                    if ui.checkbox(&mut on, label).changed() {
                        modifiers.retain(|existing| *existing != modifier);
                        if on {
                            modifiers.push(modifier);
                        }
                    }
                }
            });
        }
        Action::Macro { steps, mode } => macro_editor(ui, id, steps, mode),
        Action::Text { text } => {
            ui.horizontal(|ui| {
                ui.label("Text");
                ui.text_edit_singleline(text);
            });
        }
        Action::Launch { program, args } => {
            ui.horizontal(|ui| {
                ui.label("Program");
                ui.text_edit_singleline(program);
            });
            arguments_field(ui, (id, "arguments"), args);
        }
        Action::OpenUrl { url } => {
            ui.horizontal(|ui| {
                ui.label("Address");
                ui.text_edit_singleline(url);
            });
        }
        Action::Mouse { button } => mouse_picker(ui, (id, "mouse"), button),
        Action::SwitchProfile { name } => {
            egui::ComboBox::from_id_salt((id, "profile")).selected_text(name.as_str()).show_ui(ui, |ui| {
                for choice in &choices.profiles {
                    ui.selectable_value(name, choice.clone(), choice.as_str());
                }
            });
        }
        Action::LightingTrigger { layer, duration_ms } => {
            egui::ComboBox::from_id_salt((id, "layer")).selected_text(layer.as_str()).show_ui(ui, |ui| {
                for choice in &choices.layers {
                    ui.selectable_value(layer, choice.clone(), choice.as_str());
                }
            });
            let mut timed = duration_ms.is_some();
            if ui.checkbox(&mut timed, "Only for a set time instead of while the key is held").changed() {
                *duration_ms = timed.then_some(1500);
            }
            if let Some(milliseconds) = duration_ms {
                ui.add(egui::DragValue::new(milliseconds).range(50..=60_000).suffix(" ms"));
            }
        }
        Action::SetBrightness { percent } => {
            ui.add(egui::Slider::new(percent, 0..=100).text("Brightness (%)"));
        }
        Action::Disable
        | Action::NextProfile
        | Action::PreviousProfile
        | Action::BrightnessUp
        | Action::BrightnessDown => {}
    }
}

pub fn macro_editor(ui: &mut egui::Ui, id: &str, steps: &mut Vec<MacroStep>, mode: &mut RepeatMode) {
    egui::ComboBox::from_id_salt((id, "repeat"))
        .selected_text(match mode {
            RepeatMode::Once => "Play once",
            RepeatMode::WhileHeld => "Repeat while the key is held",
            RepeatMode::Toggle => "Repeat until pressed again",
        })
        .width(220.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(mode, RepeatMode::Once, "Play once");
            ui.selectable_value(mode, RepeatMode::WhileHeld, "Repeat while the key is held");
            ui.selectable_value(mode, RepeatMode::Toggle, "Repeat until pressed again");
        });
    if steps.is_empty() {
        ui.colored_label(egui::Color32::LIGHT_RED, "A macro needs at least one step.");
    }
    let mut pending = None;
    for (index, step) in steps.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let mut kind = StepKind::of(step);
            egui::ComboBox::from_id_salt((id, "step", index)).selected_text(kind.label()).width(170.0).show_ui(
                ui,
                |ui| {
                    for choice in StepKind::ALL {
                        ui.selectable_value(&mut kind, choice, choice.label());
                    }
                },
            );
            if kind != StepKind::of(step) {
                *step = kind.default_step();
            }
            match step {
                MacroStep::Tap(key) | MacroStep::Down(key) | MacroStep::Up(key) => {
                    key_picker(ui, (id, "key", index), key)
                }
                MacroStep::Delay(milliseconds) => {
                    ui.add(egui::DragValue::new(milliseconds).range(0..=MAX_DELAY_MS).suffix(" ms"));
                }
                MacroStep::Text(text) => {
                    ui.text_edit_singleline(text);
                }
                MacroStep::Click(button) | MacroStep::MouseDown(button) | MacroStep::MouseUp(button) => {
                    mouse_picker(ui, (id, "button", index), button);
                }
                MacroStep::Wheel(clicks) | MacroStep::Hwheel(clicks) => {
                    ui.add(egui::DragValue::new(clicks).range(-50..=50).suffix(" clicks"));
                }
            }
            for (label, op) in
                [("Move up", StepOp::MoveUp), ("Move down", StepOp::MoveDown), ("Delete step", StepOp::Delete)]
            {
                if ui.small_button(label).clicked() {
                    pending = Some((index, op));
                }
            }
        });
    }
    if let Some((index, op)) = pending {
        apply_step_op(steps, index, op);
    }
    ui.horizontal(|ui| {
        if ui.button("Add step").clicked() {
            steps.push(StepKind::Tap.default_step());
        }
        if ui.button("Add wait").clicked() {
            steps.push(StepKind::Delay.default_step());
        }
    });
}

#[cfg(test)]
mod tests {
    use actions::{Assignment, DialConfig};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::testing::harness;

    fn choices() -> Choices {
        Choices { layers: vec!["alert".into()], profiles: vec!["games".into()] }
    }

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    #[test]
    fn every_action_kind_round_trips_and_starts_valid() {
        assert_eq!(ActionKind::ALL.len(), 14);
        for kind in ActionKind::ALL {
            let action = kind.default_action(&choices());
            assert_eq!(ActionKind::of(&action), kind);
            let assignment = Assignment { key: key("g1"), action, pass_through: vec![] };
            actions::validate(&[assignment], &DialConfig::default())
                .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn defaults_use_the_first_layer_and_profile_and_survive_empty_lists() {
        assert_eq!(
            ActionKind::LightingTrigger.default_action(&choices()),
            Action::LightingTrigger { layer: "alert".into(), duration_ms: None }
        );
        assert_eq!(
            ActionKind::SwitchProfile.default_action(&choices()),
            Action::SwitchProfile { name: "games".into() }
        );
        let none = Choices { layers: vec![], profiles: vec![] };
        assert_eq!(ActionKind::SwitchProfile.default_action(&none), Action::SwitchProfile { name: String::new() });
    }

    #[test]
    fn step_kinds_round_trip() {
        assert_eq!(StepKind::ALL.len(), 10);
        for kind in StepKind::ALL {
            assert_eq!(StepKind::of(&kind.default_step()), kind);
        }
    }

    #[test]
    fn step_operations_stop_at_the_edges() {
        let mut steps = vec![MacroStep::Delay(1), MacroStep::Delay(2), MacroStep::Delay(3)];
        apply_step_op(&mut steps, 0, StepOp::MoveUp);
        apply_step_op(&mut steps, 2, StepOp::MoveDown);
        assert_eq!(steps, [MacroStep::Delay(1), MacroStep::Delay(2), MacroStep::Delay(3)]);
        apply_step_op(&mut steps, 1, StepOp::MoveDown);
        assert_eq!(steps, [MacroStep::Delay(1), MacroStep::Delay(3), MacroStep::Delay(2)]);
        apply_step_op(&mut steps, 0, StepOp::Delete);
        assert_eq!(steps, [MacroStep::Delay(3), MacroStep::Delay(2)]);
    }

    #[test]
    fn only_keys_that_can_be_sent_are_offered_as_outputs() {
        let keys = sendable_keys();
        assert!(keys.contains(&key("escape")) && keys.contains(&key("left_ctrl")));
        assert!(!keys.contains(&key("g1")), "G keys cannot be typed");
        assert!(keys.iter().all(|key| key.output().is_some()));
    }

    type MacroState = (Vec<MacroStep>, RepeatMode);

    fn harness_for(state: MacroState) -> egui_kittest::Harness<'static, MacroState> {
        egui_kittest::Harness::builder()
            .with_size(eframe::egui::Vec2::new(900.0, 600.0))
            .with_accessibility_check(false)
            .build_ui_state(|ui, state: &mut MacroState| macro_editor(ui, "test", &mut state.0, &mut state.1), state)
    }

    #[test]
    fn the_macro_editor_adds_moves_and_deletes_steps() {
        let mut harness = harness_for((vec![MacroStep::Tap(key("a")), MacroStep::Delay(50)], RepeatMode::Once));
        harness.run();
        harness.get_by_label("Add step").click();
        harness.run();
        assert_eq!(harness.state().0.len(), 3);
        harness.get_all_by_label("Delete step").next().unwrap().click();
        harness.run();
        assert_eq!(harness.state().0, [MacroStep::Delay(50), MacroStep::Tap(key("a"))]);
        harness.get_all_by_label("Move down").next().unwrap().click();
        harness.run();
        assert_eq!(harness.state().0, [MacroStep::Tap(key("a")), MacroStep::Delay(50)]);
    }

    #[test]
    fn an_empty_macro_says_what_to_do() {
        let mut harness = harness_for((vec![], RepeatMode::Once));
        harness.run();
        harness.get_by_label("A macro needs at least one step.");
    }

    #[test]
    fn the_action_editor_edits_the_chosen_action_in_place() {
        let mut model = crate::testing::online("p", &["p"]);
        model.draft.as_mut().unwrap().profile.assignments =
            vec![Assignment { key: key("g1"), action: Action::Text { text: String::new() }, pass_through: vec![] }];
        let mut harness = harness(model, |ui, model| {
            let choices = Choices { layers: vec![], profiles: vec![] };
            let draft = model.draft.as_mut().unwrap();
            action_editor(ui, "g1", &mut draft.profile.assignments[0].action, &choices);
        });
        harness.run();
        harness.get_by_label("Text");
        harness.get_by_role(eframe::egui::accesskit::Role::TextInput).click();
        harness.run();
        harness.get_by_role(eframe::egui::accesskit::Role::TextInput).type_text("hi");
        harness.run();
        let action = &harness.state().draft.as_ref().unwrap().profile.assignments[0].action;
        assert_eq!(action, &Action::Text { text: "hi".into() });
    }

    #[test]
    fn launch_arguments_can_be_typed_with_spaces_between_them() {
        let mut model = crate::testing::online("p", &["p"]);
        model.draft.as_mut().unwrap().profile.assignments = vec![Assignment {
            key: key("g1"),
            action: Action::Launch { program: "tool".into(), args: vec![] },
            pass_through: vec![],
        }];
        let mut harness = harness(model, |ui, model| {
            let choices = Choices { layers: vec![], profiles: vec![] };
            let draft = model.draft.as_mut().unwrap();
            action_editor(ui, "g1", &mut draft.profile.assignments[0].action, &choices);
        });
        harness.run();
        let fields = |harness: &egui_kittest::Harness<'_, crate::model::Model>| {
            harness.get_all_by_role(eframe::egui::accesskit::Role::TextInput).count()
        };
        assert_eq!(fields(&harness), 2);
        harness.get_all_by_role(eframe::egui::accesskit::Role::TextInput).nth(1).unwrap().click();
        harness.run();
        harness.get_all_by_role(eframe::egui::accesskit::Role::TextInput).nth(1).unwrap().type_text("-v ");
        harness.run();
        harness.get_all_by_role(eframe::egui::accesskit::Role::TextInput).nth(1).unwrap().type_text("--fast");
        harness.run();
        let action = &harness.state().draft.as_ref().unwrap().profile.assignments[0].action;
        assert_eq!(action, &Action::Launch { program: "tool".into(), args: vec!["-v".into(), "--fast".into()] });
    }
}
