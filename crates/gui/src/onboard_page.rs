use eframe::egui;
use ipc::{DeviceState, OnboardRequest, OnboardSlot, Request};

use crate::model::Model;
use crate::onboard_state::{
    Confirm, PREVIEW_OUTDATED, SaveTarget, list_error_line, preview_heading, preview_lines, restore_problem,
};

pub const HARDWARE_MODE_HINT: &str = "The keyboard plays its active stored slot whenever k100d is not controlling it: after Release to hardware mode, or when k100d quits.";
pub const RELEASED_HINT: &str =
    "The keyboard is playing its active stored slot. Take it back to change the stored slots.";

pub fn slot_title(slot: &OnboardSlot, active: Option<u32>) -> String {
    let marker = if active == Some(slot.index) { " (active)" } else { "" };
    format!("{}: {}{marker}", slot.index, slot.name)
}

pub fn pending_text(request: &OnboardRequest) -> String {
    match request {
        OnboardRequest::List => "Reading the stored slots...".into(),
        OnboardRequest::Save { profile, slot: None } => format!("Saving \"{profile}\" as a new slot..."),
        OnboardRequest::Save { profile, slot: Some(slot) } => format!("Saving \"{profile}\" over slot {slot}..."),
        OnboardRequest::Delete { slot } => format!("Deleting slot {slot}..."),
        OnboardRequest::Activate { slot } => format!("Activating slot {slot}..."),
        OnboardRequest::Backup { .. } => "Backing up the stored slots...".into(),
        OnboardRequest::Restore { from } => format!("Restoring the stored slots from {from}..."),
    }
}

pub fn onboard_page(ui: &mut egui::Ui, model: &mut Model) {
    model.list_onboard_if_needed();
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Onboard profiles");
        hardware_mode(ui, model);
        progress(ui, model);
        report(ui, model);
        slot_list(ui, model);
        slot_actions(ui, model);
        ui.separator();
        save_area(ui, model);
        ui.separator();
        backup_area(ui, model);
    });
}

fn refuse(model: &mut Model, result: Result<(), String>) {
    if let Err(message) = result {
        model.onboard.error = Some(message);
    }
}

fn hardware_mode(ui: &mut egui::Ui, model: &mut Model) {
    let device = model.state.as_ref().map(|state| state.device.clone());
    let idle = model.onboard.pending.is_none();
    match device {
        Some(DeviceState::Connected { .. }) => {
            ui.label(HARDWARE_MODE_HINT);
            if ui.add_enabled(idle, egui::Button::new("Release to hardware mode")).clicked() {
                model.send(Request::Release);
            }
        }
        Some(DeviceState::Released) => {
            ui.label(RELEASED_HINT);
            if ui.add_enabled(idle, egui::Button::new("Take the keyboard back")).clicked() {
                model.send(Request::Reacquire);
            }
        }
        _ => {
            if let Some(blocker) = model.onboard_blocker() {
                ui.label(blocker);
            }
        }
    }
}

fn progress(ui: &mut egui::Ui, model: &Model) {
    if let Some(request) = &model.onboard.pending {
        ui.label(pending_text(request));
    }
}

fn report(ui: &mut egui::Ui, model: &Model) {
    if let Some(error) = &model.onboard.error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if let Some(outcome) = &model.onboard.outcome {
        ui.label(&outcome.message);
        if let Some(backup) = &outcome.backup {
            ui.label(format!("Backup: {backup}"));
        }
        for note in &outcome.notes {
            ui.label(note);
        }
    }
}

fn slot_list(ui: &mut egui::Ui, model: &mut Model) {
    let Some(stored) = model.onboard.slots.clone() else {
        ui.weak("No slots listed yet.");
        if let Some(message) = &model.onboard.list_error {
            ui.weak(list_error_line(message));
        }
        return;
    };
    for slot in &stored.slots {
        ui.horizontal(|ui| {
            let selected = model.onboard.selected == Some(slot.index);
            if ui.selectable_label(selected, slot_title(slot, stored.active)).clicked() {
                model.onboard.select(slot.index);
            }
            ui.weak(format!("{} files, {} bytes", slot.files, slot.bytes));
        });
    }
    if let Some(free) = stored.free_bytes {
        ui.label(format!("Free onboard space: {free} bytes"));
    }
}

fn slot_actions(ui: &mut egui::Ui, model: &mut Model) {
    let idle = model.onboard_blocker().is_none();
    let (count, active) = model.onboard.slots.as_ref().map_or((0, None), |stored| (stored.slots.len(), stored.active));
    let selected = model.onboard.selected;
    ui.horizontal(|ui| {
        let can_activate = idle && selected.is_some_and(|slot| Some(slot) != active);
        if ui.add_enabled(can_activate, egui::Button::new("Activate")).clicked()
            && let Some(slot) = selected
        {
            let result = model.activate_slot(slot);
            refuse(model, result);
        }
        match model.onboard.confirm.clone() {
            Some(Confirm::Delete(slot)) => delete_confirmation(ui, model, slot, idle),
            _ => {
                let can_delete = idle && count > 1 && selected.is_some();
                if ui.add_enabled(can_delete, egui::Button::new("Delete")).clicked()
                    && let Some(slot) = selected
                {
                    model.onboard.confirm = Some(Confirm::Delete(slot));
                }
            }
        }
        if ui.add_enabled(idle, egui::Button::new("Refresh")).clicked() {
            let result = model.refresh_onboard();
            refuse(model, result);
        }
    });
    if selected.is_some() && count == 1 {
        ui.weak("The only stored slot cannot be deleted.");
    }
}

fn delete_confirmation(ui: &mut egui::Ui, model: &mut Model, slot: u32, idle: bool) {
    if ui.add_enabled(idle, egui::Button::new(format!("Confirm delete of slot {slot}"))).clicked() {
        let result = model.delete_slot(slot);
        refuse(model, result);
    }
    if ui.button("Keep it").clicked() {
        model.onboard.confirm = None;
    }
}

fn save_area(ui: &mut egui::Ui, model: &mut Model) {
    ui.heading("Save a profile to the keyboard");
    profile_to_save(ui, model);
    let source = model.save_source();
    if let Err(reason) = &source {
        ui.weak(reason);
    }
    let usable = model.onboard_blocker().is_none() && source.is_ok();
    let selected = model.onboard.selected;
    ui.horizontal(|ui| {
        if ui.add_enabled(usable, egui::Button::new("Save as a new slot")).clicked() {
            let result = model.preview_onboard_save(SaveTarget::NewSlot);
            refuse(model, result);
        }
        if ui.add_enabled(usable && selected.is_some(), egui::Button::new("Overwrite the selected slot")).clicked()
            && let Some(slot) = selected
        {
            let result = model.preview_onboard_save(SaveTarget::Over(slot));
            refuse(model, result);
        }
    });
    preview_panel(ui, model);
}

fn profile_to_save(ui: &mut egui::Ui, model: &mut Model) {
    let names = model.state.as_ref().map(|state| state.profiles.clone()).unwrap_or_default();
    let current = model.draft.as_ref().map(|draft| draft.profile.name.clone()).unwrap_or_default();
    let mut picked = None;
    egui::ComboBox::from_label("Profile to save").selected_text(current.as_str()).show_ui(ui, |ui| {
        for name in &names {
            if ui.selectable_label(*name == current, name).clicked() && *name != current {
                picked = Some(name.clone());
            }
        }
    });
    if let Some(name) = picked {
        let result = model.edit_profile(&name);
        refuse(model, result);
    }
}

fn preview_panel(ui: &mut egui::Ui, model: &mut Model) {
    let Some(preview) = model.onboard.preview.clone() else { return };
    ui.strong(preview_heading(&preview));
    for line in preview_lines(&preview.conversion) {
        ui.label(line);
    }
    egui::CollapsingHeader::new("Files to write").show(ui, |ui| {
        for file in preview.conversion.tree.manifest() {
            ui.label(format!("{}: {} bytes", file.label, file.bytes));
        }
    });
    ui.label("A backup of every stored slot is taken first.");
    let current = model.onboard_preview_is_current();
    if !current {
        ui.label(PREVIEW_OUTDATED);
    }
    let writable = current && model.onboard_blocker().is_none();
    ui.horizontal(|ui| {
        if ui.add_enabled(writable, egui::Button::new("Write to the keyboard")).clicked() {
            let result = model.confirm_onboard_save();
            refuse(model, result);
        }
        if ui.button("Cancel").clicked() {
            model.cancel_onboard_preview();
        }
    });
}

fn backup_area(ui: &mut egui::Ui, model: &mut Model) {
    let idle = model.onboard_blocker().is_none();
    ui.heading("Backup and restore");
    if ui.add_enabled(idle, egui::Button::new("Back up now")).clicked() {
        let result = model.back_up_onboard();
        refuse(model, result);
    }
    if let Some(latest) = model.onboard.latest_backup.clone() {
        ui.label(format!("Latest backup: {latest}"));
        if ui.button("Use the latest backup").clicked() {
            model.onboard.restore_from = latest;
        }
    }
    ui.add(egui::TextEdit::singleline(&mut model.onboard.restore_from).hint_text("Backup folder").desired_width(420.0));
    let problem = restore_problem(&model.onboard.restore_from);
    if let Some(problem) = problem.filter(|_| !model.onboard.restore_from.trim().is_empty()) {
        ui.weak(problem);
    }
    restore_buttons(ui, model, idle && problem.is_none());
}

fn restore_buttons(ui: &mut egui::Ui, model: &mut Model, allowed: bool) {
    if model.onboard.confirm != Some(Confirm::Restore) {
        if ui.add_enabled(allowed, egui::Button::new("Restore from this folder")).clicked() {
            model.onboard.confirm = Some(Confirm::Restore);
        }
        return;
    }
    let folder = model.onboard.restore_from.trim().to_owned();
    ui.label(format!("This replaces every stored slot with the ones in {folder}."));
    ui.horizontal(|ui| {
        if ui.add_enabled(allowed, egui::Button::new("Confirm restore")).clicked() {
            let result = model.restore_onboard(&folder);
            refuse(model, result);
        }
        if ui.button("Do not restore").clicked() {
            model.onboard.confirm = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use ipc::{DeviceState, OnboardRequest, OnboardSlot, Reply, Request};

    use super::*;
    use crate::model::{Draft, Inbox, Model};
    use crate::onboard_state::{Confirm, Outcome, SaveTarget};
    use crate::testing::{editing, harness, mixed_profile, onboard, onboard_slots, online, profile, sent};

    fn page(model: Model) -> Harness<'static, Model> {
        let mut harness = harness(model, onboard_page);
        harness.run();
        harness
    }

    fn listed(names: &[&str], active: Option<u32>) -> Model {
        let mut model = online("p", &["p"]);
        onboard_slots(&mut model, names, active);
        model
    }

    #[test]
    fn titles_mark_the_active_slot() {
        let slot = OnboardSlot { index: 1, name: "games".into(), files: 6, bytes: 412 };
        assert_eq!(slot_title(&slot, Some(1)), "1: games (active)");
        assert_eq!(slot_title(&slot, Some(0)), "1: games");
        assert_eq!(slot_title(&slot, None), "1: games");
    }

    #[test]
    fn pending_requests_say_what_they_are_doing() {
        let save = |slot| OnboardRequest::Save { profile: "games".into(), slot };
        assert_eq!(pending_text(&OnboardRequest::List), "Reading the stored slots...");
        assert_eq!(pending_text(&save(None)), "Saving \"games\" as a new slot...");
        assert_eq!(pending_text(&save(Some(2))), "Saving \"games\" over slot 2...");
        assert_eq!(pending_text(&OnboardRequest::Delete { slot: 1 }), "Deleting slot 1...");
        assert_eq!(pending_text(&OnboardRequest::Activate { slot: 1 }), "Activating slot 1...");
        assert_eq!(pending_text(&OnboardRequest::Backup { to: None }), "Backing up the stored slots...");
        assert_eq!(
            pending_text(&OnboardRequest::Restore { from: "/b/1".into() }),
            "Restoring the stored slots from /b/1..."
        );
    }

    #[test]
    fn opening_the_page_lists_the_slots_once() {
        let mut harness = page(online("p", &["p"]));
        assert_eq!(sent(harness.state_mut()), [onboard(OnboardRequest::List)]);
        harness.get_by_label("Reading the stored slots...");
        harness.run();
        assert!(sent(harness.state_mut()).is_empty());
    }

    #[test]
    fn opening_the_page_keeps_the_error_of_an_earlier_change() {
        let mut model = online("p", &["p"]);
        model.onboard.error = Some("put it back with: k100 onboard restore \"/b/1\"".into());
        let mut harness = page(model);
        assert_eq!(sent(harness.state_mut()), [onboard(OnboardRequest::List)]);
        harness.get_by_label_contains("put it back with");
    }

    #[test]
    fn slots_show_their_size_the_active_one_and_the_free_space() {
        let harness = page(listed(&["HW Profile 2", "games"], Some(1)));
        harness.get_by_label("0: HW Profile 2");
        harness.get_by_label("1: games (active)");
        assert_eq!(harness.get_all_by_label("6 files, 400 bytes").count(), 2);
        harness.get_by_label("Free onboard space: 8000000 bytes");
        harness.get_by_label(HARDWARE_MODE_HINT);
    }

    #[test]
    fn activate_acts_on_the_selected_slot_and_not_on_the_active_one() {
        let mut harness = page(listed(&["a", "b"], Some(0)));
        assert!(harness.get_by_label("Activate").accesskit_node().is_disabled());
        harness.get_by_label("0: a (active)").click();
        harness.run();
        assert!(harness.get_by_label("Activate").accesskit_node().is_disabled());
        harness.get_by_label("1: b").click();
        harness.run();
        harness.get_by_label("Activate").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [onboard(OnboardRequest::Activate { slot: 1 })]);
    }

    #[test]
    fn delete_asks_first_and_keep_it_sends_nothing() {
        let mut harness = page(listed(&["a", "b"], Some(0)));
        harness.get_by_label("1: b").click();
        harness.run();
        harness.get_by_label("Delete").click();
        harness.run();
        assert!(sent(harness.state_mut()).is_empty());
        harness.get_by_label("Keep it").click();
        harness.run();
        assert_eq!(harness.state().onboard.confirm, None);
        harness.get_by_label("Delete").click();
        harness.run();
        harness.get_by_label("Confirm delete of slot 1").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [onboard(OnboardRequest::Delete { slot: 1 })]);
    }

    #[test]
    fn selecting_another_slot_withdraws_the_delete_question() {
        let mut harness = page(listed(&["a", "b", "c"], Some(0)));
        harness.get_by_label("1: b").click();
        harness.run();
        harness.get_by_label("Delete").click();
        harness.run();
        harness.get_by_label("Confirm delete of slot 1");
        harness.get_by_label("2: c").click();
        harness.run();
        assert!(harness.query_by_label("Confirm delete of slot 1").is_none());
        assert_eq!(harness.state().onboard.confirm, None);
    }

    #[test]
    fn the_only_slot_offers_no_delete() {
        let mut harness = page(listed(&["a"], Some(0)));
        harness.get_by_label("0: a (active)").click();
        harness.run();
        assert!(harness.get_by_label("Delete").accesskit_node().is_disabled());
        harness.get_by_label("The only stored slot cannot be deleted.");
    }

    #[test]
    fn a_running_request_disables_every_action_and_says_what_it_is_doing() {
        let mut model = listed(&["a", "b"], Some(0));
        model.onboard.select(1);
        model.activate_slot(1).unwrap();
        sent(&mut model);
        let harness = page(model);
        harness.get_by_label("Activating slot 1...");
        for label in ["Activate", "Delete", "Refresh", "Back up now", "Release to hardware mode"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled(), "{label}");
        }
    }

    #[test]
    fn errors_notes_and_the_backup_path_are_shown_in_the_page() {
        let mut model = listed(&["a"], Some(0));
        model.onboard.error = Some("keyboard: no reply to WriteBufferBegin".into());
        model.onboard.outcome = Some(Outcome {
            message: "deleted slot 1 (\"b\")".into(),
            backup: Some("/b/1".into()),
            notes: vec!["file 0x6d71 was left on the keyboard".into(), "check the keyboard: 0x6e timed out".into()],
        });
        let harness = page(model);
        for label in [
            "keyboard: no reply to WriteBufferBegin",
            "deleted slot 1 (\"b\")",
            "Backup: /b/1",
            "file 0x6d71 was left on the keyboard",
            "check the keyboard: 0x6e timed out",
        ] {
            harness.get_by_label(label);
        }
    }

    #[test]
    fn a_refused_action_is_explained_in_the_page() {
        let mut model = listed(&["a"], Some(0));
        model.onboard.confirm = Some(Confirm::Delete(0));
        let mut harness = page(model);
        harness.get_by_label("Confirm delete of slot 0").click();
        harness.run();
        harness.get_by_label("The only stored slot cannot be deleted.");
        assert!(sent(harness.state_mut()).is_empty());
    }

    #[test]
    fn restore_asks_first_and_takes_the_latest_backup_on_request() {
        let folder = std::env::temp_dir().join("k100-backup").display().to_string();
        let mut model = listed(&["a"], Some(0));
        model.onboard.latest_backup = Some(folder.clone());
        let mut harness = page(model);
        harness.get_by_label(&format!("Latest backup: {folder}"));
        harness.get_by_label("Use the latest backup").click();
        harness.run();
        assert_eq!(harness.state().onboard.restore_from, folder);
        harness.get_by_label("Restore from this folder").click();
        harness.run();
        assert_eq!(harness.state().onboard.confirm, Some(Confirm::Restore));
        harness.get_by_label(&format!("This replaces every stored slot with the ones in {folder}."));
        assert!(sent(harness.state_mut()).is_empty());
        harness.get_by_label("Confirm restore").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [onboard(OnboardRequest::Restore { from: folder })]);
    }

    #[test]
    fn do_not_restore_sends_nothing() {
        let folder = std::env::temp_dir().join("k100-backup").display().to_string();
        let mut model = listed(&["a"], Some(0));
        model.onboard.restore_from = folder;
        model.onboard.confirm = Some(Confirm::Restore);
        let mut harness = page(model);
        harness.get_by_label("Do not restore").click();
        harness.run();
        assert_eq!(harness.state().onboard.confirm, None);
        assert!(sent(harness.state_mut()).is_empty());
    }

    #[test]
    fn a_relative_restore_folder_is_explained_in_the_page() {
        let mut model = listed(&["a"], Some(0));
        model.onboard.restore_from = "onboard-backup".into();
        let harness = page(model);
        harness.get_by_label_contains("full path of the backup folder");
        assert!(harness.get_by_label("Restore from this folder").accesskit_node().is_disabled());
    }

    #[test]
    fn back_up_now_and_release_send_their_requests() {
        let mut harness = page(listed(&["a"], Some(0)));
        harness.get_by_label("Back up now").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [onboard(OnboardRequest::Backup { to: None })]);
        harness.state_mut().onboard.pending = None;
        harness.run();
        harness.get_by_label("Release to hardware mode").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::Release]);
    }

    #[test]
    fn a_released_keyboard_offers_to_take_it_back_and_is_not_asked() {
        let mut model = online("p", &["p"]);
        model.state.as_mut().unwrap().device = DeviceState::Released;
        let mut harness = page(model);
        harness.get_by_label(RELEASED_HINT);
        assert!(sent(harness.state_mut()).is_empty());
        assert!(harness.get_by_label("Refresh").accesskit_node().is_disabled());
        harness.get_by_label("Take the keyboard back").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::Reacquire]);
    }

    #[test]
    fn without_a_keyboard_the_page_says_why() {
        let mut model = online("p", &["p"]);
        model.state.as_mut().unwrap().device = DeviceState::Disconnected;
        let harness = page(model);
        harness.get_by_label_contains("need a connected keyboard");
    }

    #[test]
    fn save_as_a_new_slot_previews_then_writes_after_confirmation() {
        let mut harness = page(editing(mixed_profile("p"), &["a"]));
        harness.get_by_label("Save as a new slot").click();
        harness.run();
        harness.get_by_label("Saves \"p\" as a new slot.");
        harness.get_by_label_contains("skipped: layer \"fade\"");
        harness.get_by_label("A backup of every stored slot is taken first.");
        assert!(sent(harness.state_mut()).is_empty());
        harness.get_by_label("Write to the keyboard").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [onboard(OnboardRequest::Save { profile: "p".into(), slot: None })]);
        harness.get_by_label("Saving \"p\" as a new slot...");
    }

    #[test]
    fn overwrite_names_the_slot_it_replaces_and_cancel_writes_nothing() {
        let mut harness = page(editing(profile("p"), &["a", "b"]));
        assert!(harness.get_by_label("Overwrite the selected slot").accesskit_node().is_disabled());
        harness.get_by_label("1: b").click();
        harness.run();
        harness.get_by_label("Overwrite the selected slot").click();
        harness.run();
        harness.get_by_label("Replaces slot 1 (\"b\") with \"p\".");
        harness.get_by_label("Nothing is left out.");
        harness.get_by_label("Cancel").click();
        harness.run();
        assert_eq!(harness.state().onboard.preview, None);
        assert!(sent(harness.state_mut()).is_empty());
    }

    #[test]
    fn a_stale_preview_cannot_be_written() {
        let mut model = editing(profile("p"), &["a"]);
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        model.draft = Some(Draft::new(mixed_profile("p")));
        let harness = page(model);
        harness.get_by_label("The profile or the slot list changed since this preview. Preview again.");
        assert!(harness.get_by_label("Write to the keyboard").accesskit_node().is_disabled());
    }

    #[test]
    fn unsaved_edits_show_why_saving_waits() {
        let mut model = editing(profile("p"), &["a"]);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        let harness = page(model);
        harness.get_by_label_contains("Apply your edits first");
        assert!(harness.get_by_label("Save as a new slot").accesskit_node().is_disabled());
    }

    #[test]
    fn an_overwrite_preview_cannot_be_written_once_its_slot_moved() {
        let mut model = editing(mixed_profile("p"), &["a", "b", "c"]);
        model.preview_onboard_save(SaveTarget::Over(1)).unwrap();
        onboard_slots(&mut model, &["b", "c"], Some(0));
        let mut harness = page(model);
        harness.get_by_label("The profile or the slot list changed since this preview. Preview again.");
        assert!(harness.get_by_label("Write to the keyboard").accesskit_node().is_disabled());
        assert!(sent(harness.state_mut()).is_empty());
    }

    #[test]
    fn the_profile_box_loads_another_profile_into_the_editor() {
        let mut harness = page(editing(profile("p"), &["a"]));
        harness.get_by_label("Profile to save").click();
        harness.run();
        harness.get_by_label("games").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::GetProfile { name: "games".into() }]);
    }

    #[test]
    fn the_files_to_write_are_listed_under_a_header() {
        let mut harness = page(editing(profile("p"), &["a"]));
        harness.get_by_label("Save as a new slot").click();
        harness.run();
        harness.get_by_label("Files to write").click();
        harness.run();
        let first = &onboard::convert(&profile("p")).tree.manifest()[0];
        harness.get_by_label(&format!("{}: {} bytes", first.label, first.bytes));
    }

    #[test]
    fn a_running_save_disables_previewing() {
        let mut model = editing(profile("p"), &["a"]);
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        model.confirm_onboard_save().unwrap();
        sent(&mut model);
        let harness = page(model);
        harness.get_by_label("Saving \"p\" as a new slot...");
        assert!(harness.get_by_label("Save as a new slot").accesskit_node().is_disabled());
    }

    #[test]
    fn a_failed_list_shows_the_line_and_leaves_no_slot_to_act_on() {
        let mut model = editing(mixed_profile("p"), &["a", "b"]);
        model.onboard.select(1);
        model.delete_slot(1).unwrap();
        let message = "keyboard: no reply; put it back with: k100 onboard restore \"/b/1\"";
        model
            .apply(Inbox::reply(onboard(OnboardRequest::Delete { slot: 1 }), Reply::Error { message: message.into() }));
        model.apply(Inbox::reply(
            onboard(OnboardRequest::List),
            Reply::Error { message: "no reply to the listing".into() },
        ));
        sent(&mut model);
        let harness = page(model);
        harness.get_by_label("The slot list could not be read: no reply to the listing. Click Refresh.");
        harness.get_by_label_contains("put it back with");
        harness.get_by_label("No slots listed yet.");
        for label in ["Activate", "Delete", "Overwrite the selected slot"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled(), "{label}");
        }
        assert!(harness.query_by_label("Write to the keyboard").is_none());
        assert!(!harness.get_by_label("Refresh").accesskit_node().is_disabled());
    }
}
