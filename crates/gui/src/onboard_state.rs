use std::path::Path;

use ipc::{DeviceState, OnboardRequest, OnboardSlot, Profile, Reply, Request};
use onboard::{Conversion, convert};

use crate::model::{Connection, Model, device_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSlots {
    pub slots: Vec<OnboardSlot>,
    pub active: Option<u32>,
    pub free_bytes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub message: String,
    pub backup: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Delete(u32),
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveTarget {
    NewSlot,
    Over(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub profile: Profile,
    pub target: SaveTarget,
    pub conversion: Conversion,
}

pub const APPLY_FIRST: &str = "Apply your edits first: k100d saves the profile as it was last applied.";
pub const PREVIEW_OUTDATED: &str = "The profile or the slot list changed since this preview. Preview again.";

#[derive(Debug, Default)]
pub struct OnboardForm {
    pub slots: Option<StoredSlots>,
    pub pending: Option<OnboardRequest>,
    pub outcome: Option<Outcome>,
    pub error: Option<String>,
    pub selected: Option<u32>,
    pub confirm: Option<Confirm>,
    pub restore_from: String,
    pub latest_backup: Option<String>,
    pub preview: Option<Preview>,
    list_failed: bool,
}

impl OnboardForm {
    pub fn keyboard_changed(&mut self) {
        self.slots = None;
        self.selected = None;
        self.confirm = None;
        self.preview = None;
        self.list_failed = false;
    }

    pub fn select(&mut self, slot: u32) {
        if self.selected != Some(slot) && matches!(self.confirm, Some(Confirm::Delete(_))) {
            self.confirm = None;
        }
        self.selected = Some(slot);
    }

    fn accept_slots(&mut self, stored: StoredSlots) {
        let count = stored.slots.len();
        let past_the_end = |slot: u32| usize::try_from(slot).map_or(true, |index| index >= count);
        if self.selected.is_some_and(past_the_end) {
            self.selected = None;
        }
        if matches!(self.confirm, Some(Confirm::Delete(slot)) if past_the_end(slot)) {
            self.confirm = None;
        }
        self.slots = Some(stored);
        self.list_failed = false;
    }

    fn fail(&mut self, request: &OnboardRequest, message: String) {
        if *request == OnboardRequest::List {
            self.list_failed = true;
            if self.error.is_none() && self.outcome.is_none() {
                self.error = Some(message);
            }
        } else {
            self.error = Some(message);
        }
    }
}

pub fn restore_problem(folder: &str) -> Option<&'static str> {
    let folder = folder.trim();
    if folder.is_empty() {
        Some("Type the folder of a backup.")
    } else if !Path::new(folder).is_absolute() {
        Some("Type the full path of the backup folder: k100d may run in a different folder.")
    } else {
        None
    }
}

pub fn preview_lines(conversion: &Conversion) -> Vec<String> {
    let files = conversion.tree.manifest();
    let bytes: usize = files.iter().map(|file| file.bytes).sum();
    let mut lines = vec![format!("{} files, {bytes} bytes", files.len())];
    if conversion.report.is_empty() {
        lines.push("Nothing is left out.".into());
    } else {
        lines.extend(conversion.report.lines());
    }
    lines
}

pub fn preview_heading(preview: &Preview, slots: &[OnboardSlot]) -> String {
    let name = &preview.profile.name;
    match preview.target {
        SaveTarget::NewSlot => format!("Saves \"{name}\" as a new slot."),
        SaveTarget::Over(slot) => match slots.iter().find(|listed| listed.index == slot) {
            Some(listed) => format!("Replaces slot {slot} (\"{}\") with \"{name}\".", listed.name),
            None => format!("Replaces slot {slot} with \"{name}\"."),
        },
    }
}

fn changes_the_keyboard(request: &OnboardRequest) -> bool {
    !matches!(request, OnboardRequest::List | OnboardRequest::Backup { .. })
}

impl Model {
    pub fn onboard_blocker(&self) -> Option<String> {
        let Some(state) = self.state.as_ref().filter(|_| self.connection == Connection::Online) else {
            return Some("Waiting for k100d.".into());
        };
        match &state.device {
            DeviceState::Connected { .. } => {}
            DeviceState::Released => {
                return Some("k100d has released the keyboard; take it back to change the stored slots.".into());
            }
            other => return Some(format!("The stored slots need a connected keyboard ({}).", device_text(other))),
        }
        self.onboard.pending.as_ref().map(|_| "Wait for the current keyboard request to finish.".into())
    }

    pub fn onboard_needs_listing(&self) -> bool {
        self.onboard_blocker().is_none() && self.onboard.slots.is_none() && !self.onboard.list_failed
    }

    pub fn list_onboard_if_needed(&mut self) -> bool {
        let needed = self.onboard_needs_listing();
        if needed {
            self.send_onboard(OnboardRequest::List);
        }
        needed
    }

    pub fn refresh_onboard(&mut self) -> Result<(), String> {
        self.start_onboard(OnboardRequest::List)
    }

    pub fn activate_slot(&mut self, slot: u32) -> Result<(), String> {
        self.listed_slot_count(slot)?;
        self.start_onboard(OnboardRequest::Activate { slot })
    }

    pub fn delete_slot(&mut self, slot: u32) -> Result<(), String> {
        if self.listed_slot_count(slot)? == 1 {
            return Err("The only stored slot cannot be deleted.".into());
        }
        self.start_onboard(OnboardRequest::Delete { slot })
    }

    pub fn back_up_onboard(&mut self) -> Result<(), String> {
        self.start_onboard(OnboardRequest::Backup { to: None })
    }

    pub fn restore_onboard(&mut self, folder: &str) -> Result<(), String> {
        if let Some(blocker) = self.onboard_blocker() {
            return Err(blocker);
        }
        if let Some(problem) = restore_problem(folder) {
            return Err(problem.into());
        }
        self.start_onboard(OnboardRequest::Restore { from: folder.trim().to_owned() })
    }

    pub fn save_source(&self) -> Result<Profile, String> {
        let draft = self.draft.as_ref().ok_or("No profile is loaded yet.")?;
        if draft.saving || draft.dirty() {
            return Err(APPLY_FIRST.into());
        }
        Ok(draft.saved.clone())
    }

    pub fn preview_onboard_save(&mut self, target: SaveTarget) -> Result<(), String> {
        if let Some(blocker) = self.onboard_blocker() {
            return Err(blocker);
        }
        let profile = self.save_source()?;
        if let SaveTarget::Over(slot) = target {
            self.listed_slot_count(slot)?;
        }
        self.onboard.error = None;
        let conversion = convert(&profile);
        self.onboard.preview = Some(Preview { profile, target, conversion });
        Ok(())
    }

    pub fn onboard_preview_is_current(&self) -> bool {
        let Some(preview) = &self.onboard.preview else { return false };
        self.save_source().as_ref() == Ok(&preview.profile)
            && match preview.target {
                SaveTarget::NewSlot => true,
                SaveTarget::Over(slot) => self.slot_is_listed(slot),
            }
    }

    pub fn confirm_onboard_save(&mut self) -> Result<(), String> {
        let Some(preview) = self.onboard.preview.clone() else {
            return Err("Preview the profile before writing it.".into());
        };
        if self.save_source().as_ref() != Ok(&preview.profile) {
            return Err(PREVIEW_OUTDATED.into());
        }
        let slot = match preview.target {
            SaveTarget::NewSlot => None,
            SaveTarget::Over(slot) => {
                self.listed_slot_count(slot)?;
                Some(slot)
            }
        };
        self.start_onboard(OnboardRequest::Save { profile: preview.profile.name, slot })?;
        self.onboard.preview = None;
        Ok(())
    }

    pub fn cancel_onboard_preview(&mut self) {
        self.onboard.preview = None;
    }

    pub(crate) fn settle_onboard(&mut self, request: &Request, reply: &Reply) -> bool {
        let Request::Onboard { request } = request else { return false };
        self.finish_onboard(request);
        match reply {
            Reply::OnboardSlots { slots, active, free_bytes } => {
                let stored = StoredSlots { slots: slots.clone(), active: *active, free_bytes: *free_bytes };
                self.onboard.accept_slots(stored);
            }
            Reply::OnboardDone { message, backup, notes } => {
                if backup.is_some() {
                    self.onboard.latest_backup.clone_from(backup);
                }
                let (message, backup, notes) = (message.clone(), backup.clone(), notes.clone());
                self.onboard.outcome = Some(Outcome { message, backup, notes });
                self.list_after_change(request);
            }
            Reply::Error { message } => self.onboard_failed(request, message.clone()),
            _ => {
                let message = "k100d sent an unexpected reply to an onboard request".to_owned();
                self.onboard_failed(request, message);
            }
        }
        true
    }

    pub(crate) fn onboard_request_failed(&mut self, request: &Request, message: &str) -> bool {
        let Request::Onboard { request } = request else { return false };
        self.finish_onboard(request);
        self.onboard_failed(request, message.to_owned());
        true
    }

    fn finish_onboard(&mut self, request: &OnboardRequest) {
        if self.onboard.pending.as_ref() == Some(request) {
            self.onboard.pending = None;
        }
    }

    fn onboard_failed(&mut self, request: &OnboardRequest, message: String) {
        self.onboard.fail(request, message);
        self.list_after_change(request);
    }

    fn list_after_change(&mut self, request: &OnboardRequest) {
        if changes_the_keyboard(request) && self.onboard_blocker().is_none() {
            self.send_onboard(OnboardRequest::List);
        }
    }

    fn slot_is_listed(&self, slot: u32) -> bool {
        self.onboard.slots.as_ref().is_some_and(|stored| stored.slots.iter().any(|listed| listed.index == slot))
    }

    fn listed_slot_count(&self, slot: u32) -> Result<usize, String> {
        if let Some(blocker) = self.onboard_blocker() {
            return Err(blocker);
        }
        let Some(stored) = self.onboard.slots.as_ref() else { return Err("Refresh the slot list first.".into()) };
        if !self.slot_is_listed(slot) {
            return Err(format!("There is no stored slot {slot}; refresh the list."));
        }
        Ok(stored.slots.len())
    }

    fn start_onboard(&mut self, request: OnboardRequest) -> Result<(), String> {
        if let Some(blocker) = self.onboard_blocker() {
            return Err(blocker);
        }
        self.onboard.error = None;
        self.onboard.outcome = None;
        self.onboard.confirm = None;
        self.send_onboard(request);
        Ok(())
    }

    fn send_onboard(&mut self, request: OnboardRequest) {
        self.onboard.pending = Some(request.clone());
        self.send(Request::Onboard { request });
    }
}

#[cfg(test)]
mod tests {
    use ipc::{DeviceState, Event, OnboardRequest, Reply, Request};

    use super::*;
    use crate::model::{Inbox, Model};
    use crate::testing::{editing, mixed_profile, onboard, onboard_slots, online, profile, sent};

    fn mixed() -> ipc::Profile {
        mixed_profile("p")
    }

    fn list() -> Request {
        onboard(OnboardRequest::List)
    }

    fn with_slots(names: &[&str], active: Option<u32>) -> Model {
        let mut model = online("p", &["p"]);
        onboard_slots(&mut model, names, active);
        model
    }

    fn backup_path() -> String {
        std::env::temp_dir().join("k100-backup").display().to_string()
    }

    #[test]
    fn refreshing_lists_the_slots_and_keeps_the_active_marker() {
        let mut model = online("p", &["p"]);
        assert!(model.onboard_needs_listing());
        model.refresh_onboard().unwrap();
        assert_eq!(sent(&mut model), [list()]);
        assert_eq!(model.onboard.pending, Some(OnboardRequest::List));
        onboard_slots(&mut model, &["HW Profile 2", "games"], Some(1));
        let stored = model.onboard.slots.as_ref().unwrap();
        assert_eq!((stored.slots.len(), stored.active, stored.free_bytes), (2, Some(1), Some(8_000_000)));
        assert_eq!(model.onboard.pending, None);
        assert!(!model.onboard_needs_listing());
    }

    #[test]
    fn a_released_or_missing_keyboard_is_not_asked() {
        let mut model = online("p", &["p"]);
        model.state.as_mut().unwrap().device = DeviceState::Released;
        assert!(!model.onboard_needs_listing());
        assert!(model.refresh_onboard().unwrap_err().contains("released"));
        model.state.as_mut().unwrap().device = DeviceState::Disconnected;
        assert!(model.refresh_onboard().unwrap_err().contains("No keyboard found"));
        assert!(sent(&mut model).is_empty());
    }

    #[test]
    fn a_second_request_waits_for_the_first() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.activate_slot(1).unwrap();
        assert!(model.delete_slot(1).unwrap_err().contains("Wait for the current keyboard request"));
        assert!(model.refresh_onboard().is_err());
        assert!(model.back_up_onboard().is_err());
        assert_eq!(sent(&mut model), [onboard(OnboardRequest::Activate { slot: 1 })]);
    }

    #[test]
    fn the_only_slot_cannot_be_deleted_and_unknown_slots_are_refused() {
        let mut model = with_slots(&["a"], Some(0));
        assert_eq!(model.delete_slot(0).unwrap_err(), "The only stored slot cannot be deleted.");
        assert_eq!(model.activate_slot(3).unwrap_err(), "There is no stored slot 3; refresh the list.");
        let mut unlisted = online("p", &["p"]);
        assert_eq!(unlisted.activate_slot(0).unwrap_err(), "Refresh the slot list first.");
        assert!(sent(&mut model).is_empty() && sent(&mut unlisted).is_empty());
    }

    #[test]
    fn a_finished_change_shows_its_report_and_lists_again() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.delete_slot(1).unwrap();
        sent(&mut model);
        let done = Reply::OnboardDone {
            message: "deleted slot 1 (\"b\")".into(),
            backup: Some("/cfg/onboard-backup/20261017T120000Z".into()),
            notes: vec!["file 0x6d71 was left on the keyboard".into()],
        };
        model.apply(Inbox::reply(onboard(OnboardRequest::Delete { slot: 1 }), done));
        let outcome = model.onboard.outcome.clone().unwrap();
        assert_eq!(outcome.message, "deleted slot 1 (\"b\")");
        assert_eq!(outcome.notes, ["file 0x6d71 was left on the keyboard"]);
        assert_eq!(model.onboard.latest_backup.as_deref(), Some("/cfg/onboard-backup/20261017T120000Z"));
        assert_eq!(sent(&mut model), [list()]);
        onboard_slots(&mut model, &["a"], Some(0));
        assert_eq!(model.onboard.outcome, Some(outcome), "the automatic list keeps the report");
        assert!(model.toasts.is_empty());
    }

    #[test]
    fn an_onboard_error_stays_in_the_page_and_lists_again() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.delete_slot(1).unwrap();
        sent(&mut model);
        let message = "keyboard: no reply to WriteBufferBegin; the backup taken before the change is in /b/1; \
                       put it back with: k100 onboard restore \"/b/1\"";
        model
            .apply(Inbox::reply(onboard(OnboardRequest::Delete { slot: 1 }), Reply::Error { message: message.into() }));
        assert_eq!(model.onboard.error.as_deref(), Some(message));
        assert_eq!(sent(&mut model), [list()]);
        onboard_slots(&mut model, &["a", "b"], Some(0));
        assert_eq!(model.onboard.error.as_deref(), Some(message));
        assert!(model.toasts.is_empty());
    }

    #[test]
    fn a_failed_automatic_list_keeps_the_error_and_the_report_already_shown() {
        let mut errored = with_slots(&["a", "b"], Some(0));
        errored.delete_slot(1).unwrap();
        errored.apply(Inbox::reply(
            onboard(OnboardRequest::Delete { slot: 1 }),
            Reply::Error { message: "restore with: k100 onboard restore \"/b/1\"".into() },
        ));
        errored.apply(Inbox::reply(list(), Reply::Error { message: "no reply to the listing".into() }));
        assert_eq!(errored.onboard.error.as_deref(), Some("restore with: k100 onboard restore \"/b/1\""));

        let mut reported = with_slots(&["a", "b"], Some(0));
        reported.delete_slot(1).unwrap();
        let done = Reply::OnboardDone { message: "deleted".into(), backup: Some("/b/1".into()), notes: vec![] };
        reported.apply(Inbox::reply(onboard(OnboardRequest::Delete { slot: 1 }), done));
        reported.apply(Inbox::request_failed(list(), "k100d closed the connection"));
        assert_eq!(reported.onboard.error, None);
        assert_eq!(reported.onboard.outcome.as_ref().unwrap().message, "deleted");
        assert!(!reported.onboard_needs_listing(), "a failed list is not repeated by itself");
    }

    #[test]
    fn a_failed_list_does_not_ask_again_by_itself() {
        let mut model = online("p", &["p"]);
        model.refresh_onboard().unwrap();
        sent(&mut model);
        model.apply(Inbox::reply(list(), Reply::Error { message: "no keyboard connected".into() }));
        assert_eq!(model.onboard.error.as_deref(), Some("no keyboard connected"));
        assert!(sent(&mut model).is_empty());
        assert!(!model.onboard_needs_listing());
        model.refresh_onboard().unwrap();
        assert_eq!(model.onboard.error, None);
    }

    #[test]
    fn a_keyboard_change_lets_the_page_list_again_even_after_an_error() {
        let mut model = online("p", &["p"]);
        model.refresh_onboard().unwrap();
        model.apply(Inbox::reply(list(), Reply::Error { message: "no keyboard connected".into() }));
        assert!(!model.onboard_needs_listing());
        model.apply(Inbox::Event(Event::DeviceState { state: DeviceState::Released }));
        model.apply(Inbox::Event(Event::DeviceState {
            state: DeviceState::Connected { model: "K100 RGB".into(), firmware: "1.15.61".into() },
        }));
        assert_eq!(model.onboard.error.as_deref(), Some("no keyboard connected"), "the error is not cleared");
        assert!(model.onboard_needs_listing());
    }

    #[test]
    fn the_automatic_listing_after_a_keyboard_change_keeps_the_error_with_the_backup_path() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.delete_slot(1).unwrap();
        let message = "no reply to WriteBufferBegin; put it back with: k100 onboard restore \"/b/1\"";
        model
            .apply(Inbox::reply(onboard(OnboardRequest::Delete { slot: 1 }), Reply::Error { message: message.into() }));
        sent(&mut model);
        onboard_slots(&mut model, &["a", "b"], Some(0));
        model.apply(Inbox::Event(Event::DeviceState { state: DeviceState::Released }));
        model.apply(Inbox::Event(Event::DeviceState {
            state: DeviceState::Connected { model: "K100 RGB".into(), firmware: "1.15.61".into() },
        }));
        assert!(model.onboard_needs_listing());
        assert!(model.list_onboard_if_needed());
        assert_eq!(sent(&mut model), [list()]);
        assert_eq!(model.onboard.error.as_deref(), Some(message));
        onboard_slots(&mut model, &["a"], Some(0));
        assert_eq!(model.onboard.error.as_deref(), Some(message));
        assert!(!model.list_onboard_if_needed());
        assert!(sent(&mut model).is_empty());
    }

    #[test]
    fn the_automatic_listing_after_a_reconnect_keeps_the_report() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.delete_slot(1).unwrap();
        let done =
            Reply::OnboardDone { message: "deleted".into(), backup: Some("/b/1".into()), notes: vec!["note".into()] };
        model.apply(Inbox::reply(onboard(OnboardRequest::Delete { slot: 1 }), done));
        sent(&mut model);
        onboard_slots(&mut model, &["a"], Some(0));
        model.apply(Inbox::Disconnected("gone".into()));
        model.apply(Inbox::Connected);
        model.apply(Inbox::reply(Request::GetState, Reply::State { state: crate::testing::state("p", &["p"]) }));
        assert!(model.list_onboard_if_needed());
        onboard_slots(&mut model, &["a"], Some(0));
        assert_eq!(model.onboard.outcome.as_ref().map(|outcome| outcome.message.as_str()), Some("deleted"));
    }

    #[test]
    fn an_explicit_refresh_still_clears_the_last_error() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.onboard.error = Some("old".into());
        model.refresh_onboard().unwrap();
        assert_eq!(model.onboard.error, None);
    }

    #[test]
    fn an_unexpected_reply_is_an_error_in_the_page() {
        let mut model = online("p", &["p"]);
        model.refresh_onboard().unwrap();
        model.apply(Inbox::reply(list(), Reply::Done));
        assert!(model.onboard.error.as_deref().unwrap().contains("unexpected reply"));
        assert_eq!(model.onboard.pending, None);
    }

    #[test]
    fn restore_needs_a_full_folder_path() {
        let mut model = with_slots(&["a"], Some(0));
        assert_eq!(model.restore_onboard("  ").unwrap_err(), "Type the folder of a backup.");
        assert!(model.restore_onboard("onboard-backup/1").unwrap_err().contains("full path"));
        assert!(sent(&mut model).is_empty());
        let folder = backup_path();
        model.restore_onboard(&format!("  {folder} ")).unwrap();
        assert_eq!(sent(&mut model), [onboard(OnboardRequest::Restore { from: folder })]);
    }

    #[test]
    fn back_up_asks_k100d_for_a_new_folder_and_remembers_it() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.back_up_onboard().unwrap();
        assert_eq!(sent(&mut model), [onboard(OnboardRequest::Backup { to: None })]);
        let done =
            Reply::OnboardDone { message: "backed up 2 slots".into(), backup: Some("/b/2".into()), notes: vec![] };
        model.apply(Inbox::reply(onboard(OnboardRequest::Backup { to: None }), done));
        assert_eq!(model.onboard.latest_backup.as_deref(), Some("/b/2"));
        assert!(sent(&mut model).is_empty(), "a backup changes nothing, so no new list");
    }

    #[test]
    fn a_new_request_clears_the_confirmation_and_the_last_report() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.onboard.confirm = Some(Confirm::Delete(1));
        model.onboard.error = Some("old".into());
        model.onboard.outcome = Some(Outcome { message: "old".into(), backup: None, notes: vec![] });
        model.activate_slot(1).unwrap();
        assert_eq!(
            (model.onboard.confirm.clone(), model.onboard.error.clone(), model.onboard.outcome.clone()),
            (None, None, None)
        );
    }

    #[test]
    fn losing_the_connection_forgets_the_slots_but_waits_for_the_pending_request() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.activate_slot(1).unwrap();
        model.apply(Inbox::Disconnected("k100d closed the connection".into()));
        assert_eq!(model.onboard.slots, None);
        assert_eq!(model.onboard.pending, Some(OnboardRequest::Activate { slot: 1 }));
        model.apply(Inbox::Connected);
        assert_eq!(model.onboard.pending, Some(OnboardRequest::Activate { slot: 1 }));
        sent(&mut model);
        model
            .apply(Inbox::request_failed(onboard(OnboardRequest::Activate { slot: 1 }), "k100d closed the connection"));
        assert_eq!(model.onboard.pending, None);
        assert_eq!(model.onboard.error.as_deref(), Some("k100d closed the connection"));
    }

    #[test]
    fn a_request_lost_in_transport_is_reported_in_the_page_and_the_slots_are_read_again() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.activate_slot(1).unwrap();
        sent(&mut model);
        model
            .apply(Inbox::request_failed(onboard(OnboardRequest::Activate { slot: 1 }), "k100d closed the connection"));
        assert_eq!(model.onboard.error.as_deref(), Some("k100d closed the connection"));
        assert_eq!(model.onboard.pending, Some(OnboardRequest::List));
        assert!(model.toasts.is_empty());
        assert_eq!(sent(&mut model), [list()]);
    }

    #[test]
    fn a_lost_listing_or_backup_is_not_followed_by_another_listing() {
        let mut listing = online("p", &["p"]);
        listing.refresh_onboard().unwrap();
        sent(&mut listing);
        listing.apply(Inbox::request_failed(list(), "k100d closed the connection"));
        assert_eq!(listing.onboard.error.as_deref(), Some("k100d closed the connection"));
        assert!(sent(&mut listing).is_empty());
        assert!(!listing.onboard_needs_listing());

        let mut backup = with_slots(&["a"], Some(0));
        backup.back_up_onboard().unwrap();
        sent(&mut backup);
        backup.apply(Inbox::request_failed(onboard(OnboardRequest::Backup { to: None }), "gone"));
        assert_eq!(backup.onboard.error.as_deref(), Some("gone"));
        assert!(sent(&mut backup).is_empty());
    }

    #[test]
    fn a_keyboard_change_forgets_the_slots() {
        let mut model = with_slots(&["a", "b"], Some(0));
        model.onboard.selected = Some(1);
        model.onboard.confirm = Some(Confirm::Delete(1));
        model.apply(Inbox::Event(Event::DeviceState { state: DeviceState::Disconnected }));
        assert_eq!(
            (model.onboard.slots.clone(), model.onboard.selected, model.onboard.confirm.clone()),
            (None, None, None)
        );
    }

    #[test]
    fn an_unchanged_device_state_keeps_the_slots() {
        let mut model = with_slots(&["a", "b"], Some(0));
        let device = model.state.as_ref().unwrap().device.clone();
        model.apply(Inbox::Event(Event::DeviceState { state: device }));
        assert!(model.onboard.slots.is_some());
    }

    #[test]
    fn a_shorter_list_drops_a_selection_past_its_end() {
        let mut model = with_slots(&["a", "b", "c"], Some(0));
        model.onboard.selected = Some(2);
        model.onboard.confirm = Some(Confirm::Delete(2));
        onboard_slots(&mut model, &["a", "b"], Some(0));
        assert_eq!((model.onboard.selected, model.onboard.confirm.clone()), (None, None));
        model.onboard.selected = Some(1);
        onboard_slots(&mut model, &["a", "b"], Some(0));
        assert_eq!(model.onboard.selected, Some(1));
    }

    #[test]
    fn choosing_another_slot_drops_a_pending_delete_confirmation() {
        let mut form = OnboardForm { selected: Some(1), confirm: Some(Confirm::Delete(1)), ..OnboardForm::default() };
        form.select(1);
        assert_eq!(form.confirm, Some(Confirm::Delete(1)));
        form.select(0);
        assert_eq!((form.selected, form.confirm.clone()), (Some(0), None));
        form.confirm = Some(Confirm::Restore);
        form.select(1);
        assert_eq!(form.confirm, Some(Confirm::Restore));
    }

    #[test]
    fn a_preview_converts_the_applied_profile_without_touching_the_keyboard() {
        let mut model = editing(mixed(), &["a"]);
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        assert!(sent(&mut model).is_empty());
        let preview = model.onboard.preview.clone().unwrap();
        assert_eq!(preview.conversion, onboard::convert(&mixed()));
        let lines = preview_lines(&preview.conversion);
        assert!(lines.iter().any(|line| line.starts_with("skipped: layer \"fade\"")), "{lines:?}");
        assert_eq!(
            preview_heading(&preview, &model.onboard.slots.as_ref().unwrap().slots),
            "Saves \"p\" as a new slot."
        );
    }

    #[test]
    fn preview_lines_sum_the_files_and_say_when_nothing_is_left_out() {
        let conversion = onboard::convert(&profile("p"));
        let files = conversion.tree.manifest();
        let bytes: usize = files.iter().map(|file| file.bytes).sum();
        assert_eq!(
            preview_lines(&conversion),
            [format!("{} files, {bytes} bytes", files.len()), "Nothing is left out.".into()]
        );
    }

    #[test]
    fn confirming_a_preview_saves_by_name_to_the_chosen_slot() {
        let mut model = editing(mixed(), &["a", "b"]);
        model.preview_onboard_save(SaveTarget::Over(1)).unwrap();
        let preview = model.onboard.preview.clone().unwrap();
        assert_eq!(
            preview_heading(&preview, &model.onboard.slots.as_ref().unwrap().slots),
            "Replaces slot 1 (\"b\") with \"p\"."
        );
        model.confirm_onboard_save().unwrap();
        assert_eq!(sent(&mut model), [onboard(OnboardRequest::Save { profile: "p".into(), slot: Some(1) })]);
        assert_eq!(model.onboard.preview, None);
        assert!(model.confirm_onboard_save().is_err(), "the preview is used up");
    }

    #[test]
    fn a_refused_confirmation_keeps_the_preview() {
        let mut model = editing(mixed(), &["a"]);
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        model.onboard.pending = Some(OnboardRequest::List);
        assert!(model.confirm_onboard_save().unwrap_err().contains("Wait for the current keyboard request"));
        assert!(model.onboard.preview.is_some());
        assert!(sent(&mut model).is_empty());
    }

    #[test]
    fn unsaved_edits_block_saving_to_the_keyboard() {
        let mut model = editing(mixed(), &["a"]);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        assert!(model.preview_onboard_save(SaveTarget::NewSlot).unwrap_err().starts_with("Apply your edits first"));
        model.draft.as_mut().unwrap().profile.apps.clear();
        model.draft.as_mut().unwrap().saving = true;
        assert!(model.save_source().is_err());
        model.draft = None;
        assert_eq!(model.save_source().unwrap_err(), "No profile is loaded yet.");
    }

    #[test]
    fn a_preview_of_an_older_profile_cannot_be_written() {
        let mut model = editing(profile("p"), &["a"]);
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        model.apply(Inbox::reply(Request::GetProfile { name: "p".into() }, Reply::Profile { profile: mixed() }));
        assert!(!model.onboard_preview_is_current());
        assert!(model.confirm_onboard_save().unwrap_err().ends_with("Preview again."));
        assert!(sent(&mut model).is_empty());
    }

    #[test]
    fn overwriting_a_slot_that_is_gone_is_refused() {
        let mut model = editing(mixed(), &["a", "b"]);
        assert_eq!(
            model.preview_onboard_save(SaveTarget::Over(5)).unwrap_err(),
            "There is no stored slot 5; refresh the list."
        );
        model.preview_onboard_save(SaveTarget::Over(1)).unwrap();
        onboard_slots(&mut model, &["a"], Some(0));
        assert!(!model.onboard_preview_is_current());
        assert!(model.confirm_onboard_save().is_err());
        assert!(sent(&mut model).is_empty());
    }

    #[test]
    fn a_new_preview_clears_the_last_error_and_a_cancel_drops_it() {
        let mut model = editing(mixed(), &["a"]);
        model.onboard.error = Some("old".into());
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        assert_eq!(model.onboard.error, None);
        model.cancel_onboard_preview();
        assert_eq!(model.onboard.preview, None);
    }

    #[test]
    fn a_keyboard_change_forgets_the_preview() {
        let mut model = editing(mixed(), &["a"]);
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        model.apply(Inbox::Event(Event::DeviceState { state: DeviceState::Disconnected }));
        assert_eq!(model.onboard.preview, None);
    }

    #[test]
    fn a_finished_save_shows_the_daemons_notes_and_lists_again() {
        let mut model = editing(mixed(), &["a"]);
        model.preview_onboard_save(SaveTarget::NewSlot).unwrap();
        model.confirm_onboard_save().unwrap();
        sent(&mut model);
        let save = onboard(OnboardRequest::Save { profile: "p".into(), slot: None });
        let done = Reply::OnboardDone {
            message: "saved \"p\" to slot 1 (5 files, 300 bytes)".into(),
            backup: Some("/b/3".into()),
            notes: vec!["skipped: layer \"fade\": gradient has no hardware layout".into()],
        };
        model.apply(Inbox::reply(save, done));
        assert_eq!(model.onboard.outcome.as_ref().unwrap().notes.len(), 1);
        assert_eq!(sent(&mut model), [list()]);
    }
}
