use actions::{Action, DialMode, DialModeKind, Key};
use eframe::egui::{self, Sense, Vec2};

use crate::actions_editor::{ActionKind, Choices, action_editor};
use crate::colors::{color_button, to_color32};
use crate::model::{Model, move_item};

pub const BUILTIN: [DialModeKind; 8] = [
    DialModeKind::Brightness,
    DialModeKind::MacroRecording,
    DialModeKind::MediaJogging,
    DialModeKind::TrackSelector,
    DialModeKind::VerticalScroll,
    DialModeKind::HorizontalScroll,
    DialModeKind::Zoom,
    DialModeKind::SwitchApps,
];

pub fn kind_label(kind: DialModeKind) -> &'static str {
    match kind {
        DialModeKind::Brightness => "Brightness",
        DialModeKind::MacroRecording => "Macro recording",
        DialModeKind::MediaJogging => "Media jogging",
        DialModeKind::TrackSelector => "Track selector",
        DialModeKind::VerticalScroll => "Vertical scroll",
        DialModeKind::HorizontalScroll => "Horizontal scroll",
        DialModeKind::Zoom => "Zoom",
        DialModeKind::SwitchApps => "Switch applications",
        DialModeKind::Custom => "Custom",
    }
}

pub fn kind_help(kind: DialModeKind) -> &'static str {
    match kind {
        DialModeKind::Brightness => "Turn to change the keyboard brightness.",
        DialModeKind::MacroRecording => "Hold the dial to start recording a macro.",
        DialModeKind::MediaJogging => "Turn to seek with the left and right arrow keys.",
        DialModeKind::TrackSelector => "Turn for the next or previous track; hold to play or pause.",
        DialModeKind::VerticalScroll => "Turn to scroll up and down.",
        DialModeKind::HorizontalScroll => "Turn to scroll sideways.",
        DialModeKind::Zoom => "Turn to zoom in and out.",
        DialModeKind::SwitchApps => "Turn to move through open windows like Alt+Tab.",
        DialModeKind::Custom => "Choose what turning and holding the dial does.",
    }
}

pub fn mode_title(mode: &DialMode) -> String {
    match mode.name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => kind_label(mode.kind).to_owned(),
    }
}

pub fn available_kinds(modes: &[DialMode]) -> Vec<DialModeKind> {
    let mut kinds: Vec<DialModeKind> =
        BUILTIN.into_iter().filter(|kind| !modes.iter().any(|mode| mode.kind == *kind)).collect();
    kinds.push(DialModeKind::Custom);
    kinds
}

fn volume(name: &str) -> Action {
    Action::Remap { key: Key::named(name).expect("built-in key names are in the key table"), modifiers: Vec::new() }
}

pub fn add_mode(modes: &mut Vec<DialMode>, kind: DialModeKind) -> usize {
    let mut mode = DialMode::builtin(kind);
    if kind == DialModeKind::Custom {
        let mut number = 1;
        while modes.iter().any(|existing| existing.label() == format!("Custom {number}")) {
            number += 1;
        }
        mode.name = Some(format!("Custom {number}"));
        mode.clockwise = Some(volume("volume_up"));
        mode.counter_clockwise = Some(volume("volume_down"));
    }
    modes.push(mode);
    modes.len() - 1
}

pub fn remove_mode(modes: &mut Vec<DialMode>, index: usize) -> bool {
    if modes.len() <= 1 || index >= modes.len() {
        return false;
    }
    modes.remove(index);
    true
}

pub fn dial(ui: &mut egui::Ui, model: &mut Model) {
    let (current, profiles) =
        model.state.as_ref().map(|state| (state.dial_mode.clone(), state.profiles.clone())).unwrap_or_default();
    let Some(draft) = model.draft.as_mut() else { return };
    let choices = Choices { layers: draft.profile.layers.iter().map(|layer| layer.name.clone()).collect(), profiles };
    let modes = &mut draft.profile.dial.modes;
    ui.label("Press the dial to cycle through these modes in this order. The ring shows the mode's colour.");
    ui.add_space(6.0);
    ui.columns(2, |columns| {
        mode_list(&mut columns[0], modes, &mut model.dial, &current);
        mode_editor(&mut columns[1], modes, model.dial, &choices);
    });
}

fn mode_list(ui: &mut egui::Ui, modes: &mut Vec<DialMode>, selected: &mut usize, current: &str) {
    egui::ComboBox::from_label("Add mode").selected_text("Choose a mode").show_ui(ui, |ui| {
        for kind in available_kinds(modes) {
            if ui.selectable_label(false, kind_label(kind)).clicked() {
                *selected = add_mode(modes, kind);
            }
        }
    });
    *selected = (*selected).min(modes.len().saturating_sub(1));
    for (index, mode) in modes.iter().enumerate() {
        ui.horizontal(|ui| {
            let (swatch, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            ui.painter().rect_filled(swatch, 3, to_color32(mode.color()));
            let mut title = mode_title(mode);
            if mode.label() == current {
                title.push_str(" (current)");
            }
            if ui.selectable_label(*selected == index, title).clicked() {
                *selected = index;
            }
        });
    }
    ui.horizontal(|ui| {
        let index = *selected;
        if ui.button("Earlier").clicked()
            && let Some(moved) = move_item(modes, index, false)
        {
            *selected = moved;
        }
        if ui.button("Later").clicked()
            && let Some(moved) = move_item(modes, index, true)
        {
            *selected = moved;
        }
        if ui.add_enabled(modes.len() > 1, egui::Button::new("Remove mode")).clicked() {
            remove_mode(modes, index);
            *selected = index.saturating_sub(1);
        }
    });
}

fn mode_editor(ui: &mut egui::Ui, modes: &mut [DialMode], selected: usize, choices: &Choices) {
    let Some(mode) = modes.get_mut(selected) else { return };
    ui.strong(mode_title(mode));
    ui.label(kind_help(mode.kind));
    ui.horizontal(|ui| {
        ui.label("Ring colour");
        let mut color = mode.color();
        if color_button(ui, &mut color) {
            mode.color = Some(color);
        }
        if ui.add_enabled(mode.color.is_some(), egui::Button::new("Default colour")).clicked() {
            mode.color = None;
        }
    });
    if mode.kind != DialModeKind::Custom {
        return;
    }
    ui.horizontal(|ui| {
        ui.label("Name");
        name_field(ui, &mut mode.name);
    });
    slot(ui, "Turning clockwise", &format!("dial-{selected}-clockwise"), &mut mode.clockwise, choices);
    slot(ui, "Turning counter-clockwise", &format!("dial-{selected}-counter"), &mut mode.counter_clockwise, choices);
    slot(ui, "Holding the dial", &format!("dial-{selected}-hold"), &mut mode.press, choices);
    if mode.actions().next().is_none() {
        ui.colored_label(egui::Color32::LIGHT_RED, "A custom mode needs at least one action.");
    }
}

fn name_field(ui: &mut egui::Ui, name: &mut Option<String>) {
    let Some(text) = name.as_mut() else {
        let mut typed = String::new();
        if ui.text_edit_singleline(&mut typed).changed() {
            *name = Some(typed);
        }
        return;
    };
    let response = ui.text_edit_singleline(text);
    if response.lost_focus() && text.trim().is_empty() {
        *name = None;
    }
}

fn slot(ui: &mut egui::Ui, label: &str, id: &str, slot: &mut Option<Action>, choices: &Choices) {
    let mut on = slot.is_some();
    if ui.checkbox(&mut on, label).changed() {
        *slot = on.then(|| ActionKind::Remap.default_action(choices));
    }
    if let Some(action) = slot {
        ui.indent(id, |ui| action_editor(ui, id, action, choices));
    }
}

#[cfg(test)]
mod tests {
    use actions::{Action, DialConfig, Key};
    use eframe::egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::model::{AUTO_APPLY_QUIET, Model};
    use crate::testing::{harness, online, sent};

    fn modes(model: &Model) -> &Vec<DialMode> {
        &model.draft.as_ref().unwrap().profile.dial.modes
    }

    fn kinds(model: &Model) -> Vec<DialModeKind> {
        modes(model).iter().map(|mode| mode.kind).collect()
    }

    #[test]
    fn only_unused_built_in_modes_and_custom_can_be_added() {
        let mut list = DialConfig::default().modes;
        let offered = available_kinds(&list);
        assert!(!offered.contains(&DialModeKind::Brightness));
        assert!(offered.contains(&DialModeKind::Zoom) && offered.contains(&DialModeKind::Custom));
        for kind in BUILTIN {
            if !list.iter().any(|mode| mode.kind == kind) {
                add_mode(&mut list, kind);
            }
        }
        assert_eq!(available_kinds(&list), [DialModeKind::Custom]);
    }

    #[test]
    fn custom_modes_get_unique_names_and_are_valid_straight_away() {
        let mut list = DialConfig::default().modes;
        let first = add_mode(&mut list, DialModeKind::Custom);
        let second = add_mode(&mut list, DialModeKind::Custom);
        assert_eq!((list[first].label(), list[second].label()), ("Custom 1".to_owned(), "Custom 2".to_owned()));
        assert_eq!(
            list[first].clockwise,
            Some(Action::Remap { key: Key::named("volume_up").unwrap(), modifiers: vec![] })
        );
        actions::validate(&[], &DialConfig { modes: list }).unwrap();
    }

    #[test]
    fn a_taken_custom_name_is_skipped() {
        let mut list = DialConfig::default().modes;
        let first = add_mode(&mut list, DialModeKind::Custom);
        list[first].name = Some("Custom 2".into());
        let second = add_mode(&mut list, DialModeKind::Custom);
        assert_eq!(list[second].label(), "Custom 1");
        let third = add_mode(&mut list, DialModeKind::Custom);
        assert_eq!(list[third].label(), "Custom 3");
    }

    #[test]
    fn the_last_mode_cannot_be_removed() {
        let mut list = vec![DialMode::builtin(DialModeKind::Zoom)];
        assert!(!remove_mode(&mut list, 0));
        list.push(DialMode::builtin(DialModeKind::Brightness));
        assert!(!remove_mode(&mut list, 5));
        assert!(remove_mode(&mut list, 0));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, DialModeKind::Brightness);
    }

    #[test]
    fn titles_prefer_the_custom_name() {
        let mut mode = DialMode::builtin(DialModeKind::SwitchApps);
        assert_eq!(mode_title(&mode), "Switch applications");
        mode.kind = DialModeKind::Custom;
        mode.name = Some("Volume".into());
        assert_eq!(mode_title(&mode), "Volume");
        mode.name = Some("  ".into());
        assert_eq!(mode_title(&mode), "Custom");
    }

    #[test]
    fn the_pane_lists_modes_marks_the_current_one_and_adds_more() {
        let mut model = online("p", &["p"]);
        model.state.as_mut().unwrap().dial_mode = "brightness".into();
        let mut harness = harness(model, dial);
        harness.run();
        harness.get_by_label("Brightness (current)");
        harness.get_by_label("Track selector");
        harness.get_by_label("Add mode").click();
        harness.run();
        harness.get_by_label("Zoom").click();
        harness.run();
        assert_eq!(kinds(harness.state()).last(), Some(&DialModeKind::Zoom));
        assert_eq!(harness.state().dial, 4);
    }

    #[test]
    fn earlier_moves_the_selected_mode_and_the_selection_follows() {
        let mut harness = harness(online("p", &["p"]), dial);
        harness.run();
        harness.get_by_label("Track selector").click();
        harness.run();
        harness.get_by_label("Earlier").click();
        harness.run();
        assert_eq!(
            kinds(harness.state()),
            [
                DialModeKind::Brightness,
                DialModeKind::MacroRecording,
                DialModeKind::TrackSelector,
                DialModeKind::MediaJogging
            ]
        );
        assert_eq!(harness.state().dial, 2);
    }

    #[test]
    fn later_moves_the_selected_mode_and_stops_at_the_end() {
        let mut harness = harness(online("p", &["p"]), dial);
        harness.run();
        harness.get_by_label("Later").click();
        harness.run();
        assert_eq!(kinds(harness.state())[1], DialModeKind::Brightness);
        assert_eq!(harness.state().dial, 1);
        harness.state_mut().dial = 3;
        harness.run();
        let before = kinds(harness.state());
        harness.get_by_label("Later").click();
        harness.run();
        assert_eq!(kinds(harness.state()), before);
        assert_eq!(harness.state().dial, 3);
    }

    #[test]
    fn removing_selects_the_neighbour() {
        let mut harness = harness(online("p", &["p"]), dial);
        harness.run();
        harness.get_by_label("Track selector").click();
        harness.run();
        harness.get_by_label("Remove mode").click();
        harness.run();
        assert_eq!(
            kinds(harness.state()),
            [DialModeKind::Brightness, DialModeKind::MacroRecording, DialModeKind::MediaJogging]
        );
        assert_eq!(harness.state().dial, 2);
    }

    #[test]
    fn removing_the_first_mode_keeps_the_selection_on_the_new_first() {
        let mut harness = harness(online("p", &["p"]), dial);
        harness.run();
        harness.get_by_label("Remove mode").click();
        harness.run();
        assert_eq!(modes(harness.state())[0].kind, DialModeKind::MacroRecording);
        assert_eq!(harness.state().dial, 0);
    }

    #[test]
    fn remove_is_disabled_for_the_only_mode() {
        let mut model = online("p", &["p"]);
        model.draft.as_mut().unwrap().profile.dial.modes = vec![DialMode::builtin(DialModeKind::Brightness)];
        let mut harness = harness(model, dial);
        harness.run();
        assert!(harness.get_by_label("Remove mode").accesskit_node().is_disabled());
    }

    #[test]
    fn a_stale_selection_is_clamped_to_the_list() {
        let mut model = online("p", &["p"]);
        model.dial = 9;
        let mut harness = harness(model, dial);
        harness.run();
        assert_eq!(harness.state().dial, 3);
    }

    #[test]
    fn a_custom_mode_without_actions_is_flagged_for_the_draft_bar() {
        let mut model = online("p", &["p"]);
        let draft = model.draft.as_mut().unwrap();
        let index = add_mode(&mut draft.profile.dial.modes, DialModeKind::Custom);
        model.dial = index;
        let mut harness = harness(model, dial);
        harness.run();
        harness.get_by_label("Turning clockwise").click();
        harness.run();
        harness.get_by_label("Turning counter-clockwise").click();
        harness.run();
        assert!(modes(harness.state())[index].actions().next().is_none());
        assert!(harness.state().draft_problem().unwrap().contains("custom dial mode"));
        harness.get_by_label("Holding the dial").click();
        harness.run();
        assert!(modes(harness.state())[index].press.is_some());
        assert!(harness.state().draft_problem().is_none());
    }

    #[test]
    fn the_ring_colour_can_return_to_the_default() {
        let mut model = online("p", &["p"]);
        model.draft.as_mut().unwrap().profile.dial.modes[0].color = Some(k100::Rgb::new(1, 2, 3));
        let mut harness = harness(model, dial);
        harness.run();
        harness.get_by_label("Default colour").click();
        harness.run();
        assert_eq!(modes(harness.state())[0].color, None);
    }

    #[test]
    fn typing_a_custom_name_edits_the_draft_and_keeps_the_text() {
        let mut model = online("p", &["p"]);
        let draft = model.draft.as_mut().unwrap();
        model.dial = add_mode(&mut draft.profile.dial.modes, DialModeKind::Custom);
        let mut harness = harness(model, dial);
        harness.run();
        harness.get_by_role(Role::TextInput).click();
        harness.run();
        harness.get_by_role(Role::TextInput).type_text("x");
        harness.run();
        assert_eq!(modes(harness.state())[4].name.as_deref(), Some("Custom 1x"));
        harness.get_by_role(Role::TextInput).type_text("y");
        harness.run();
        assert_eq!(modes(harness.state())[4].name.as_deref(), Some("Custom 1xy"));
    }

    #[test]
    fn editing_the_dial_saves_the_profile_once_it_is_valid() {
        let model = online("p", &["p"]);
        let mut harness = harness(model, dial);
        harness.run();
        harness.get_by_label("Track selector").click();
        harness.run();
        harness.get_by_label("Earlier").click();
        harness.run();
        harness.state_mut().tick(std::time::Duration::ZERO);
        harness.state_mut().tick(AUTO_APPLY_QUIET * 3);
        let requests = sent(harness.state_mut());
        assert!(requests.iter().any(|request| matches!(request, ipc::Request::SaveProfile { .. })));
    }
}
