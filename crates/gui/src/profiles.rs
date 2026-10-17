use eframe::egui;
use ipc::{Profile, Reply, Request, SettingsPatch};

use crate::layers::{EffectKind, new_layer};
use crate::model::{Draft, Model};

const RECORDING_HOLD: &str = "Finish the macro recording first: profile changes are held while recording.";

#[derive(Debug, Default)]
pub struct ProfilesForm {
    pub new_name: String,
    pub rename_to: String,
    pub app_text: String,
    pub confirm_delete: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRename {
    pub old: String,
    pub new: String,
    pub was_active: bool,
    pub was_default: bool,
}

pub fn name_problem(name: &str, existing: &[String]) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        Some("Type a name first.".into())
    } else if existing.iter().any(|taken| taken == name) {
        Some(format!("A profile called {name:?} already exists."))
    } else {
        None
    }
}

pub fn profile_title(name: &str, active: &str, default: &str) -> String {
    let mut marks = Vec::new();
    if name == active {
        marks.push("active");
    }
    if name == default {
        marks.push("default");
    }
    if marks.is_empty() { name.to_owned() } else { format!("{name} ({})", marks.join(", ")) }
}

pub fn add_app(apps: &mut Vec<String>, text: &str) -> bool {
    let program = text.trim();
    if program.is_empty() || apps.iter().any(|existing| existing.eq_ignore_ascii_case(program)) {
        return false;
    }
    apps.push(program.to_owned());
    true
}

pub fn new_profile(name: &str) -> Profile {
    Profile {
        version: 1,
        name: name.to_owned(),
        apps: Vec::new(),
        layers: vec![new_layer(&[], EffectKind::RainbowWave)],
        assignments: Vec::new(),
        dial: actions::DialConfig::default(),
    }
}

pub fn duplicate_profile(source: &Profile, name: &str) -> Profile {
    Profile { name: name.to_owned(), apps: Vec::new(), ..source.clone() }
}

impl Model {
    fn profile_names(&self) -> Vec<String> {
        self.state.as_ref().map(|state| state.profiles.clone()).unwrap_or_default()
    }

    fn free_name(&self, name: &str) -> Result<String, String> {
        if self.recording() {
            return Err(RECORDING_HOLD.into());
        }
        let name = name.trim();
        match name_problem(name, &self.profile_names()) {
            Some(problem) => Err(problem),
            None => Ok(name.to_owned()),
        }
    }

    pub fn renaming(&self) -> bool {
        self.pending_rename.is_some()
    }

    pub fn create_profile(&mut self, name: &str) -> Result<(), String> {
        let name = self.free_name(name)?;
        self.send(Request::SaveProfile { profile: new_profile(&name) });
        Ok(())
    }

    pub fn duplicate_draft(&mut self, name: &str) -> Result<(), String> {
        let name = self.free_name(name)?;
        let Some(draft) = self.draft.as_ref() else { return Err("No profile is loaded yet.".into()) };
        let copy = duplicate_profile(&draft.saved, &name);
        self.send(Request::SaveProfile { profile: copy });
        Ok(())
    }

    pub fn rename_draft(&mut self, name: &str) -> Result<(), String> {
        if self.recording() {
            return Err(RECORDING_HOLD.into());
        }
        if self.renaming() {
            return Err("That profile is already being renamed.".into());
        }
        let name = self.free_name(name)?;
        let Some(draft) = self.draft.as_ref() else { return Err("No profile is loaded yet.".into()) };
        if draft.dirty() {
            return Err("Apply or revert your edits before renaming.".into());
        }
        let old = draft.profile.name.clone();
        let renamed = Profile { name: name.clone(), ..draft.profile.clone() };
        let was_active = old == self.active_profile();
        let was_default = self.settings.as_ref().is_some_and(|settings| settings.default_profile == old);
        self.pending_rename = Some(PendingRename { old, new: name, was_active, was_default });
        self.send(Request::SaveProfile { profile: renamed });
        Ok(())
    }

    pub(crate) fn settle_rename(&mut self, request: &Request, reply: &Reply) -> bool {
        let Request::SaveProfile { profile } = request else { return false };
        if !self.pending_rename.as_ref().is_some_and(|pending| pending.new == profile.name) {
            return false;
        }
        let Some(pending) = self.pending_rename.take() else { return false };
        match reply {
            Reply::Done => self.finish_rename(&pending, profile),
            Reply::Error { message } => self.toast(message.clone()),
            _ => self.toast(format!("The daemon did not confirm renaming {} to {}.", pending.old, pending.new)),
        }
        true
    }

    fn finish_rename(&mut self, pending: &PendingRename, saved: &Profile) {
        if pending.was_active {
            self.send(Request::SetActiveProfile { name: pending.new.clone() });
        }
        if pending.was_default {
            let patch = SettingsPatch { default_profile: Some(pending.new.clone()), ..SettingsPatch::default() };
            self.send(Request::SetSettings { patch });
        }
        self.send(Request::DeleteProfile { name: pending.old.clone() });
        if let Some(draft) = self.draft.as_mut()
            && draft.profile.name == pending.old
        {
            draft.profile.name = pending.new.clone();
            draft.saved = saved.clone();
            draft.saving = false;
            draft.rejected = None;
        }
    }

    pub fn edit_profile(&mut self, name: &str) -> Result<(), String> {
        if self.recording() {
            return Err(RECORDING_HOLD.into());
        }
        if self.draft.as_ref().is_some_and(Draft::dirty) {
            return Err("Apply or revert your edits first.".into());
        }
        self.name_edit = None;
        self.send(Request::GetProfile { name: name.to_owned() });
        Ok(())
    }

    pub fn delete_profile(&mut self, name: &str) -> Result<(), String> {
        if name == self.active_profile() {
            return Err("The active profile cannot be deleted; switch to another one first.".into());
        }
        self.send(Request::DeleteProfile { name: name.to_owned() });
        Ok(())
    }
}

pub fn profiles(ui: &mut egui::Ui, model: &mut Model) {
    let Some(state) = model.state.clone() else { return };
    let default = model.settings.as_ref().map(|settings| settings.default_profile.clone()).unwrap_or_default();
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |columns| {
            profile_list(&mut columns[0], model, &state.profiles, &state.active_profile, &default);
            profile_editor(&mut columns[1], model);
        });
    });
}

fn pressed_enter(response: &egui::Response) -> bool {
    response.lost_focus() && response.ctx.input(|input| input.key_pressed(egui::Key::Enter))
}

fn profile_list(ui: &mut egui::Ui, model: &mut Model, names: &[String], active: &str, default: &str) {
    ui.strong("Profiles");
    let pick = match model.draft.as_ref() {
        Some(draft) => names.iter().find(|name| **name == draft.profile.name).cloned(),
        None => Some(active.to_owned()),
    };
    for name in names {
        let picked = pick.as_ref() == Some(name);
        if ui.selectable_label(picked, profile_title(name, active, default)).clicked() && !picked {
            match model.edit_profile(name) {
                Ok(()) => model.profiles_form.confirm_delete = None,
                Err(problem) => model.toast(problem),
            }
        }
    }
    let pick = pick.unwrap_or_default();
    let listed = !pick.is_empty();
    ui.horizontal(|ui| {
        if ui.add_enabled(listed && pick != active, egui::Button::new("Switch to")).clicked() {
            model.send(Request::SetActiveProfile { name: pick.clone() });
        }
        if ui.add_enabled(listed && pick != default, egui::Button::new("Make default")).clicked() {
            let patch = SettingsPatch { default_profile: Some(pick.clone()), ..SettingsPatch::default() };
            model.send(Request::SetSettings { patch });
        }
    });
    if listed {
        delete_controls(ui, model, &pick, active);
    } else {
        ui.add_enabled(false, egui::Button::new("Delete"));
    }
    ui.separator();
    new_profile_controls(ui, model, names);
}

fn delete_controls(ui: &mut egui::Ui, model: &mut Model, pick: &str, active: &str) {
    if model.profiles_form.confirm_delete.as_deref() == Some(pick) {
        ui.horizontal(|ui| {
            if ui.button("Confirm delete").clicked() {
                model.profiles_form.confirm_delete = None;
                if let Err(problem) = model.delete_profile(pick) {
                    model.toast(problem);
                }
            }
            if ui.button("Keep it").clicked() {
                model.profiles_form.confirm_delete = None;
            }
        });
        return;
    }
    let deletable = pick != active;
    if ui.add_enabled(deletable, egui::Button::new("Delete")).clicked() {
        model.profiles_form.confirm_delete = Some(pick.to_owned());
    }
    if !deletable {
        ui.weak("The active profile cannot be deleted; switch to another one first.");
    }
}

fn new_profile_controls(ui: &mut egui::Ui, model: &mut Model, names: &[String]) {
    ui.strong("New profile");
    let field = ui.add(egui::TextEdit::singleline(&mut model.profiles_form.new_name).hint_text("Name"));
    let typed = model.profiles_form.new_name.clone();
    let problem = name_problem(&typed, names);
    let usable = problem.is_none();
    let mut create = pressed_enter(&field) && usable;
    let mut duplicate = false;
    ui.horizontal(|ui| {
        create |= ui.add_enabled(usable, egui::Button::new("Create empty")).clicked();
        let can_copy = usable && model.draft.is_some();
        duplicate = ui.add_enabled(can_copy, egui::Button::new("Duplicate the edited profile")).clicked();
    });
    if create || duplicate {
        let result = if create { model.create_profile(&typed) } else { model.duplicate_draft(&typed) };
        match result {
            Ok(()) => model.profiles_form.new_name.clear(),
            Err(message) => model.toast(message),
        }
    }
    if !typed.trim().is_empty()
        && let Some(problem) = problem
    {
        ui.weak(problem);
    }
}

fn profile_editor(ui: &mut egui::Ui, model: &mut Model) {
    let Some(name) = model.draft.as_ref().map(|draft| draft.profile.name.clone()) else {
        ui.label("Loading the profile...");
        return;
    };
    ui.strong(format!("Editing {name}"));
    rename_controls(ui, model);
    ui.separator();
    linked_programs(ui, model);
}

fn rename_controls(ui: &mut egui::Ui, model: &mut Model) {
    if let Some(pending) = model.pending_rename.as_ref() {
        ui.weak(format!("Renaming {} to {}...", pending.old, pending.new));
        return;
    }
    let mut rename = false;
    ui.horizontal(|ui| {
        let field = ui.add(egui::TextEdit::singleline(&mut model.profiles_form.rename_to).hint_text("New name"));
        rename = ui.button("Rename").clicked() || pressed_enter(&field);
    });
    if rename {
        let target = model.profiles_form.rename_to.clone();
        match model.rename_draft(&target) {
            Ok(()) => model.profiles_form.rename_to.clear(),
            Err(message) => model.toast(message),
        }
    }
}

fn linked_programs(ui: &mut egui::Ui, model: &mut Model) {
    ui.strong("Programs that switch to this profile");
    ui.weak("Use the program's file name, for example game.exe, or its full path.");
    let mut remove = None;
    if let Some(draft) = model.draft.as_ref() {
        for (index, app) in draft.profile.apps.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(app.as_str());
                if ui.small_button(format!("Remove {app}")).clicked() {
                    remove = Some(index);
                }
            });
        }
    }
    if let (Some(index), Some(draft)) = (remove, model.draft.as_mut()) {
        draft.profile.apps.remove(index);
    }
    let mut add = false;
    ui.horizontal(|ui| {
        let field = ui.add(egui::TextEdit::singleline(&mut model.profiles_form.app_text).hint_text("Program name"));
        add = ui.button("Add program").clicked() || pressed_enter(&field);
    });
    if add {
        let text = model.profiles_form.app_text.clone();
        if let Some(draft) = model.draft.as_mut()
            && add_app(&mut draft.profile.apps, &text)
        {
            model.profiles_form.app_text.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use actions::RecorderState;
    use eframe::egui::{self, accesskit::Role};
    use egui_kittest::kittest::{NodeT, Queryable};
    use ipc::{Reply, Request, Settings, SettingsPatch};

    use super::*;
    use crate::model::{Inbox, validate_profile};
    use crate::testing::{harness, online, profile, sent};

    fn model() -> Model {
        let mut model = online("p", &["games", "p"]);
        model.settings = Some(Settings { default_profile: "p".into(), ..Settings::default() });
        model
    }

    fn names(existing: &[&str]) -> Vec<String> {
        existing.iter().map(|name| (*name).to_owned()).collect()
    }

    fn renamed(name: &str) -> Request {
        Request::SaveProfile { profile: profile(name) }
    }

    fn make_default(name: &str) -> Request {
        let patch = SettingsPatch { default_profile: Some(name.into()), ..SettingsPatch::default() };
        Request::SetSettings { patch }
    }

    fn switch_to(name: &str) -> Request {
        Request::SetActiveProfile { name: name.into() }
    }

    fn delete(name: &str) -> Request {
        Request::DeleteProfile { name: name.into() }
    }

    fn start_recording(model: &mut Model) {
        model.state.as_mut().unwrap().recording = RecorderState::AwaitingKey;
    }

    #[test]
    fn names_must_be_new_and_non_empty() {
        let existing = names(&["p", "games"]);
        assert!(name_problem("", &existing).is_some());
        assert!(name_problem("   ", &existing).is_some());
        assert!(name_problem("games", &existing).unwrap().contains("already exists"));
        assert!(name_problem(" games ", &existing).unwrap().contains("already exists"));
        assert!(name_problem("  work ", &existing).is_none());
    }

    #[test]
    fn titles_mark_the_active_and_default_profiles() {
        assert_eq!(profile_title("p", "p", "p"), "p (active, default)");
        assert_eq!(profile_title("p", "p", "x"), "p (active)");
        assert_eq!(profile_title("g", "p", "g"), "g (default)");
        assert_eq!(profile_title("g", "p", "x"), "g");
    }

    #[test]
    fn programs_are_trimmed_and_never_duplicated() {
        let mut apps = vec!["Game.exe".to_owned()];
        assert!(!add_app(&mut apps, "  "));
        assert!(!add_app(&mut apps, "game.EXE"));
        assert!(add_app(&mut apps, " notepad.exe "));
        assert_eq!(apps, ["Game.exe", "notepad.exe"]);
    }

    #[test]
    fn duplicates_start_without_linked_programs() {
        let mut source = profile("p");
        source.apps = vec!["game.exe".into()];
        let copy = duplicate_profile(&source, "p2");
        assert_eq!(copy.name, "p2");
        assert!(copy.apps.is_empty());
        assert_eq!(copy.layers, source.layers);
    }

    #[test]
    fn a_new_profile_has_one_rainbow_layer_and_passes_validation() {
        let fresh = new_profile("fresh");
        assert_eq!(fresh.name, "fresh");
        assert_eq!(fresh.layers.len(), 1);
        assert!(fresh.apps.is_empty());
        validate_profile(&fresh).unwrap();
    }

    #[test]
    fn creating_and_duplicating_send_a_save_and_reject_bad_names() {
        let mut model = model();
        model.create_profile(" work ").unwrap();
        let requests = sent(&mut model);
        let [Request::SaveProfile { profile }] = requests.as_slice() else { panic!("expected a save") };
        assert_eq!(profile.name, "work");
        assert_eq!(profile.layers.len(), 1);
        assert!(model.create_profile("games").is_err());
        assert!(model.duplicate_draft("").is_err());
        assert!(sent(&mut model).is_empty());
        model.draft.as_mut().unwrap().saved.apps = vec!["game.exe".into()];
        model.draft.as_mut().unwrap().profile.apps = vec!["game.exe".into(), "edited.exe".into()];
        model.duplicate_draft("p copy").unwrap();
        let requests = sent(&mut model);
        let [Request::SaveProfile { profile }] = requests.as_slice() else { panic!("expected a save") };
        assert_eq!(profile.name, "p copy");
        assert!(profile.apps.is_empty());
    }

    #[test]
    fn nothing_is_created_duplicated_or_renamed_while_a_macro_is_being_recorded() {
        let mut model = model();
        start_recording(&mut model);
        assert!(model.create_profile("work").unwrap_err().contains("recording"));
        assert!(model.duplicate_draft("work").unwrap_err().contains("recording"));
        assert!(model.rename_draft("work").unwrap_err().contains("recording"));
        assert!(sent(&mut model).is_empty());
        assert!(model.pending_rename.is_none());
    }

    #[test]
    fn renaming_only_saves_the_new_name_until_the_daemon_accepts_it() {
        let mut model = model();
        model.rename_draft(" work ").unwrap();
        let requests = sent(&mut model);
        assert!(matches!(requests.as_slice(), [Request::SaveProfile { profile }] if profile.name == "work"));
        assert_eq!(model.draft.as_ref().unwrap().profile.name, "p");
        let pending = model.pending_rename.as_ref().unwrap();
        assert_eq!((pending.old.as_str(), pending.new.as_str()), ("p", "work"));
        assert!(pending.was_active && pending.was_default);
    }

    #[test]
    fn an_accepted_rename_moves_the_active_and_default_marks_before_deleting_the_old_name() {
        let mut model = model();
        model.rename_draft("work").unwrap();
        sent(&mut model);
        model.apply(Inbox::reply(renamed("work"), Reply::Done));
        assert_eq!(sent(&mut model), [switch_to("work"), make_default("work"), delete("p")]);
        assert!(model.pending_rename.is_none());
        let draft = model.draft.as_ref().unwrap();
        assert_eq!(draft.profile.name, "work");
        assert!(!draft.dirty());
    }

    #[test]
    fn an_accepted_rename_skips_the_marks_the_profile_did_not_have() {
        let mut not_default = model();
        not_default.settings = Some(Settings { default_profile: "games".into(), ..Settings::default() });
        not_default.rename_draft("work").unwrap();
        sent(&mut not_default);
        not_default.apply(Inbox::reply(renamed("work"), Reply::Done));
        assert_eq!(sent(&mut not_default), [switch_to("work"), delete("p")]);

        let mut neither = model();
        neither.state.as_mut().unwrap().active_profile = "games".into();
        neither.settings = Some(Settings { default_profile: "games".into(), ..Settings::default() });
        neither.rename_draft("work").unwrap();
        sent(&mut neither);
        neither.apply(Inbox::reply(renamed("work"), Reply::Done));
        assert_eq!(sent(&mut neither), [delete("p")]);
    }

    #[test]
    fn a_refused_rename_sends_nothing_more_and_keeps_the_old_profile() {
        let mut model = model();
        model.rename_draft("work").unwrap();
        sent(&mut model);
        model.apply(Inbox::reply(renamed("work"), Reply::Error { message: "disk full".into() }));
        assert!(sent(&mut model).is_empty());
        assert_eq!(model.toasts, ["disk full"]);
        assert!(model.pending_rename.is_none());
        let draft = model.draft.as_ref().unwrap();
        assert_eq!(draft.profile.name, "p");
        assert!(draft.rejected.is_none() && !draft.saving);
    }

    #[test]
    fn saves_wait_for_the_rename_and_edits_made_meanwhile_follow_the_new_name() {
        let mut model = model();
        model.rename_draft("work").unwrap();
        sent(&mut model);
        model.draft.as_mut().unwrap().profile.apps.push("game.exe".into());
        assert!(!model.apply_draft());
        assert!(model.rename_draft("other").unwrap_err().contains("already being renamed"));
        model.apply(Inbox::reply(renamed("work"), Reply::Done));
        sent(&mut model);
        let draft = model.draft.as_ref().unwrap();
        assert_eq!(draft.profile.name, "work");
        assert_eq!(draft.profile.apps, ["game.exe"]);
        assert!(draft.dirty());
        assert!(model.apply_draft());
        assert!(matches!(sent(&mut model).as_slice(), [Request::SaveProfile { profile }] if profile.name == "work"));
    }

    #[test]
    fn losing_the_daemon_abandons_a_pending_rename() {
        let mut model = model();
        model.rename_draft("work").unwrap();
        model.apply(Inbox::Disconnected("gone".into()));
        assert!(model.pending_rename.is_none());
    }

    #[test]
    fn renaming_needs_a_free_name_and_no_unsaved_edits() {
        let mut model = model();
        assert!(model.rename_draft("games").unwrap_err().contains("already exists"));
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        assert!(model.rename_draft("work").unwrap_err().contains("Apply or revert"));
        assert!(sent(&mut model).is_empty());
        assert!(model.pending_rename.is_none());
    }

    #[test]
    fn editing_another_profile_is_refused_while_edits_are_unsaved() {
        let mut model = model();
        model.edit_profile("games").unwrap();
        assert_eq!(sent(&mut model), [Request::GetProfile { name: "games".into() }]);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        assert!(model.edit_profile("games").is_err());
        assert!(sent(&mut model).is_empty());
    }

    fn highlighted(harness: &egui_kittest::Harness<'_, Model>, label: &str) -> bool {
        harness.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
    }

    fn load(harness: &mut egui_kittest::Harness<'_, Model>, name: &str) {
        let request = Request::GetProfile { name: name.into() };
        assert_eq!(sent(harness.state_mut()), std::slice::from_ref(&request));
        harness.state_mut().apply(Inbox::reply(request, Reply::Profile { profile: profile(name) }));
        harness.run();
    }

    #[test]
    fn clicking_a_profile_in_the_list_loads_it_into_the_editor() {
        let mut harness = harness(model(), profiles);
        harness.run();
        harness.get_by_label("Editing p");
        assert!(harness.query_by_label("Edit").is_none(), "the list click replaces the Edit button");
        harness.get_by_label("games").click();
        harness.run();
        let request = Request::GetProfile { name: "games".into() };
        assert_eq!(sent(harness.state_mut()), std::slice::from_ref(&request));
        let mut loaded = profile("games");
        loaded.apps = vec!["game.exe".into()];
        harness.state_mut().apply(Inbox::reply(request, Reply::Profile { profile: loaded.clone() }));
        harness.run();
        harness.get_by_label("Editing games");
        assert!(highlighted(&harness, "games"));
        assert!(!highlighted(&harness, "p (active, default)"));
        harness.get_by_label("game.exe");
        let draft = harness.state().draft.as_ref().unwrap();
        assert_eq!(draft.profile, loaded);
        assert!(!draft.dirty());
    }

    #[test]
    fn a_draft_missing_from_the_list_highlights_no_profile() {
        let mut model = model();
        model.draft = Some(Draft::new(profile("gone")));
        let mut harness = harness(model, profiles);
        harness.run();
        harness.get_by_label("Editing gone");
        assert!(!highlighted(&harness, "p (active, default)"));
        assert!(!highlighted(&harness, "games"));
        for button in ["Switch to", "Make default", "Delete"] {
            assert!(harness.get_by_label(button).accesskit_node().is_disabled(), "{button}");
        }
    }

    #[test]
    fn another_profile_is_not_loaded_while_a_macro_is_recorded() {
        let mut model = model();
        start_recording(&mut model);
        assert!(model.edit_profile("games").unwrap_err().contains("recording"));
        assert!(sent(&mut model).is_empty());
    }

    #[test]
    fn unsaved_edits_keep_the_list_on_the_edited_profile() {
        let mut model = model();
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        let mut harness = harness(model, profiles);
        harness.run();
        harness.get_by_label("games").click();
        harness.run();
        assert!(sent(harness.state_mut()).is_empty());
        assert_eq!(harness.state().toasts, ["Apply or revert your edits first."]);
        assert!(highlighted(&harness, "p (active, default)"));
        assert!(!highlighted(&harness, "games"));
        harness.get_by_label("Editing p");
    }

    #[test]
    fn the_list_switches_and_sets_the_default() {
        let mut harness = harness(model(), profiles);
        harness.run();
        harness.get_by_label("p (active, default)");
        assert!(harness.get_by_label("Switch to").accesskit_node().is_disabled());
        assert!(harness.get_by_label("Make default").accesskit_node().is_disabled());
        harness.get_by_label("games").click();
        harness.run();
        load(&mut harness, "games");
        harness.get_by_label("Switch to").click();
        harness.run();
        harness.get_by_label("Make default").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [switch_to("games"), make_default("games")]);
    }

    #[test]
    fn the_active_profile_cannot_be_deleted_and_others_need_a_second_click() {
        let mut harness = harness(model(), profiles);
        harness.run();
        assert!(harness.get_by_label("Delete").accesskit_node().is_disabled());
        harness.get_by_label("games").click();
        harness.run();
        load(&mut harness, "games");
        harness.get_by_label("Delete").click();
        harness.run();
        assert!(sent(harness.state_mut()).is_empty(), "the first click only asks");
        harness.get_by_label("Keep it").click();
        harness.run();
        assert!(sent(harness.state_mut()).is_empty());
        harness.get_by_label("Delete").click();
        harness.run();
        harness.get_by_label("Confirm delete").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [delete("games")]);
        assert!(harness.query_by_label("Confirm delete").is_none());
    }

    #[test]
    fn deleting_the_edited_profile_reloads_the_active_one() {
        let mut model = model();
        model.draft = Some(Draft::new(profile("games")));
        model.delete_profile("games").unwrap();
        sent(&mut model);
        model.apply(Inbox::reply(delete("games"), Reply::Done));
        assert!(model.draft.is_none());
        assert_eq!(sent(&mut model), [Request::GetState]);
        model.apply(Inbox::reply(Request::GetState, Reply::State { state: crate::testing::state("p", &["p"]) }));
        assert_eq!(sent(&mut model), [Request::GetProfile { name: "p".into() }]);
    }

    #[test]
    fn picking_another_row_drops_a_pending_delete_confirmation() {
        let mut model = model();
        model.state.as_mut().unwrap().profiles.push("work".into());
        let mut harness = harness(model, profiles);
        harness.run();
        harness.get_by_label("games").click();
        harness.run();
        load(&mut harness, "games");
        harness.get_by_label("Delete").click();
        harness.run();
        harness.get_by_label("work").click();
        harness.run();
        load(&mut harness, "work");
        assert!(harness.query_by_label("Confirm delete").is_none());
        assert!(sent(harness.state_mut()).is_empty());
    }

    #[test]
    fn the_model_refuses_to_delete_the_active_profile() {
        let mut model = model();
        assert!(model.delete_profile("p").unwrap_err().contains("active"));
        assert!(sent(&mut model).is_empty());
        model.delete_profile("games").unwrap();
        assert_eq!(sent(&mut model), [delete("games")]);
    }

    #[test]
    fn create_is_disabled_until_the_name_is_usable() {
        let mut harness = harness(model(), profiles);
        harness.run();
        assert!(harness.get_by_label("Create empty").accesskit_node().is_disabled());
        harness.state_mut().profiles_form.new_name = "work".into();
        harness.run();
        assert!(!harness.get_by_label("Create empty").accesskit_node().is_disabled());
        harness.state_mut().profiles_form.new_name = "games".into();
        harness.run();
        assert!(harness.get_by_label("Create empty").accesskit_node().is_disabled());
        harness.get_by_label_contains("already exists");
    }

    #[test]
    fn creating_from_the_pane_sends_the_save_and_clears_the_field() {
        let mut harness = harness(model(), profiles);
        harness.run();
        harness.state_mut().profiles_form.new_name = "work".into();
        harness.run();
        harness.get_by_label("Create empty").click();
        harness.run();
        assert!(
            matches!(sent(harness.state_mut()).as_slice(), [Request::SaveProfile { profile }] if profile.name == "work")
        );
        assert!(harness.state().profiles_form.new_name.is_empty());
    }

    #[test]
    fn renaming_from_the_pane_sends_one_save_and_clears_the_field() {
        let mut harness = harness(model(), profiles);
        harness.run();
        harness.state_mut().profiles_form.rename_to = "work".into();
        harness.run();
        harness.get_by_label("Rename").click();
        harness.run();
        assert!(
            matches!(sent(harness.state_mut()).as_slice(), [Request::SaveProfile { profile }] if profile.name == "work")
        );
        assert!(harness.state().profiles_form.rename_to.is_empty());
        harness.get_by_label_contains("Renaming");
    }

    #[test]
    fn linked_programs_are_added_and_removed_in_the_draft() {
        let mut model = model();
        model.draft.as_mut().unwrap().profile.apps = vec!["game.exe".into()];
        let mut harness = harness(model, profiles);
        harness.run();
        harness.state_mut().profiles_form.app_text = "notepad.exe".into();
        harness.run();
        harness.get_by_label("Add program").click();
        harness.run();
        assert_eq!(harness.state().draft.as_ref().unwrap().profile.apps, ["game.exe", "notepad.exe"]);
        assert!(harness.state().profiles_form.app_text.is_empty());
        harness.get_by_label("Remove game.exe").click();
        harness.run();
        assert_eq!(harness.state().draft.as_ref().unwrap().profile.apps, ["notepad.exe"]);
    }

    #[test]
    fn a_duplicate_program_is_kept_in_the_field_and_not_added() {
        let mut model = model();
        model.draft.as_mut().unwrap().profile.apps = vec!["game.exe".into()];
        let mut harness = harness(model, profiles);
        harness.run();
        harness.state_mut().profiles_form.app_text = "GAME.exe".into();
        harness.run();
        harness.get_by_label("Add program").click();
        harness.run();
        assert_eq!(harness.state().draft.as_ref().unwrap().profile.apps, ["game.exe"]);
    }

    #[test]
    fn enter_in_the_program_field_adds_the_program() {
        let mut harness = harness(model(), profiles);
        harness.run();
        let fields = harness.get_all_by_role(Role::TextInput).count();
        assert_eq!(fields, 3);
        let program_field = harness.get_all_by_role(Role::TextInput).last().unwrap();
        program_field.click();
        harness.run();
        harness.get_all_by_role(Role::TextInput).last().unwrap().type_text("calc.exe");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(harness.state().draft.as_ref().unwrap().profile.apps, ["calc.exe"]);
        assert!(harness.state().profiles_form.app_text.is_empty());
    }
}
