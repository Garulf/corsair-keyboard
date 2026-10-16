use std::collections::BTreeSet;
use std::time::Duration;

use actions::RecorderState;
use engine::{Layout, Zone};
use ipc::{DaemonState, DeviceState, Event, Profile, Reply, Request, Settings};
use k100::{Frame, Rgb};

use crate::canvas::CanvasState;
use crate::device::DeviceForm;
use crate::profiles::{PendingRename, ProfilesForm};

pub const AUTO_APPLY_QUIET: Duration = Duration::from_millis(400);
const MAX_TOASTS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Offline(String),
    Online,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Overview,
    Lighting,
    Keys,
    Dial,
    Device,
    Profiles,
}

impl Pane {
    pub const ALL: &[Pane] = &[Pane::Overview, Pane::Lighting, Pane::Keys, Pane::Dial, Pane::Device, Pane::Profiles];

    pub fn label(self) -> &'static str {
        match self {
            Pane::Overview => "Overview",
            Pane::Lighting => "Lighting",
            Pane::Keys => "Keys",
            Pane::Dial => "Dial",
            Pane::Device => "Device",
            Pane::Profiles => "Profiles",
        }
    }

    pub fn edits_profile(self) -> bool {
        match self {
            Pane::Overview => false,
            Pane::Lighting => true,
            Pane::Keys => true,
            Pane::Dial => true,
            Pane::Device => false,
            Pane::Profiles => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inbox {
    Connected,
    Disconnected(String),
    LaunchFailed(String),
    LaunchFinished,
    RequestFailed(Box<Request>, String),
    Reply(Box<(Request, Reply)>),
    Event(Event),
}

impl Inbox {
    pub fn reply(request: Request, reply: Reply) -> Self {
        Self::Reply(Box::new((request, reply)))
    }

    pub fn request_failed(request: Request, message: impl Into<String>) -> Self {
        Self::RequestFailed(Box::new(request), message.into())
    }

    pub fn as_reply(&self) -> Option<(&Request, &Reply)> {
        match self {
            Self::Reply(pair) => Some((&pair.0, &pair.1)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send(Request),
    StartDaemon,
    CloseWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Closing {
    #[default]
    Open,
    Asking,
    SavingThenClose,
    Confirmed,
}

pub fn close_allowed(closing: Closing, unsaved: bool) -> bool {
    !unsaved || closing == Closing::Confirmed
}

#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub profile: Profile,
    pub saved: Profile,
    pub saving: bool,
    pub rejected: Option<Profile>,
    seen: Profile,
    edited_at: Duration,
}

impl Draft {
    pub fn new(profile: Profile) -> Self {
        Self {
            saved: profile.clone(),
            seen: profile.clone(),
            profile,
            saving: false,
            rejected: None,
            edited_at: Duration::ZERO,
        }
    }

    pub fn dirty(&self) -> bool {
        self.profile != self.saved
    }

    pub fn refused(&self) -> bool {
        self.rejected.as_ref() == Some(&self.profile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub message: String,
    pub drops_macro: bool,
}

impl Stale {
    pub fn changed_outside(name: &str) -> Self {
        Self { message: format!("{name} changed outside this window."), drops_macro: false }
    }

    pub fn macro_conflict(key: &str, name: &str) -> Self {
        Self {
            message: format!("A macro was recorded into {key}, but {name} also changed outside this window."),
            drops_macro: true,
        }
    }
}

pub fn validate_profile(profile: &Profile) -> Result<(), String> {
    if profile.name.trim().is_empty() {
        return Err("the profile needs a name".into());
    }
    let mut names = BTreeSet::new();
    for layer in &profile.layers {
        if layer.name.trim().is_empty() {
            return Err("every layer needs a name".into());
        }
        if !names.insert(layer.name.as_str()) {
            return Err(format!("two layers are called {:?}", layer.name));
        }
        if matches!(&layer.zone, Zone::Leds(leds) if leds.is_empty()) {
            return Err(format!("layer {:?} has an empty zone", layer.name));
        }
    }
    profile.validate()
}

fn without_key(profile: &Profile, key: &str) -> Profile {
    let mut copy = profile.clone();
    copy.assignments.retain(|assignment| assignment.key.name() != key);
    copy
}

fn take_assignment(target: &mut Profile, source: &Profile, key: &str) {
    target.assignments.retain(|assignment| assignment.key.name() != key);
    let at = source.assignments.iter().position(|assignment| assignment.key.name() == key);
    let at = at.unwrap_or(target.assignments.len()).min(target.assignments.len());
    let recorded = source.assignments.iter().filter(|assignment| assignment.key.name() == key).cloned();
    target.assignments.splice(at..at, recorded);
}

pub fn move_item<T>(items: &mut [T], index: usize, raise: bool) -> Option<usize> {
    if index >= items.len() {
        return None;
    }
    let target =
        if raise { index.checked_add(1).filter(|target| *target < items.len())? } else { index.checked_sub(1)? };
    items.swap(index, target);
    Some(target)
}

pub struct Model {
    pub connection: Connection,
    pub state: Option<DaemonState>,
    pub settings: Option<Settings>,
    pub draft: Option<Draft>,
    pub stale: Option<Stale>,
    pub frame: Option<Frame>,
    pub layout: Layout,
    pub selection: BTreeSet<u8>,
    pub pane: Pane,
    pub toasts: Vec<String>,
    pub starting_daemon: bool,
    pub auto_apply: bool,
    pub canvas: CanvasState,
    pub layer: usize,
    pub dial: usize,
    pub name_edit: Option<(String, String)>,
    pub paint: Rgb,
    pub device_form: DeviceForm,
    pub profiles_form: ProfilesForm,
    pub pending_rename: Option<PendingRename>,
    pending_macro: Option<String>,
    pub closing: Closing,
    effects: Vec<Effect>,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub fn new() -> Self {
        Self {
            connection: Connection::Connecting,
            state: None,
            settings: None,
            draft: None,
            stale: None,
            frame: None,
            layout: Layout::ansi(),
            selection: BTreeSet::new(),
            pane: Pane::default(),
            toasts: Vec::new(),
            starting_daemon: false,
            auto_apply: true,
            canvas: CanvasState::default(),
            layer: 0,
            dial: 0,
            name_edit: None,
            paint: Rgb::new(255, 255, 255),
            device_form: DeviceForm::default(),
            profiles_form: ProfilesForm::default(),
            pending_rename: None,
            pending_macro: None,
            closing: Closing::Open,
            effects: Vec::new(),
        }
    }

    pub fn send(&mut self, request: Request) {
        self.effects.push(Effect::Send(request));
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn request_start_daemon(&mut self) {
        if !self.starting_daemon {
            self.starting_daemon = true;
            self.effects.push(Effect::StartDaemon);
        }
    }

    pub fn toast(&mut self, message: impl Into<String>) {
        self.toasts.push(message.into());
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    pub fn active_profile(&self) -> &str {
        self.state.as_ref().map_or("", |state| state.active_profile.as_str())
    }

    pub fn draft_problem(&self) -> Option<String> {
        self.draft.as_ref().and_then(|draft| validate_profile(&draft.profile).err())
    }

    pub fn recording(&self) -> bool {
        self.state.as_ref().is_some_and(|state| state.recording != RecorderState::Idle)
    }

    pub fn can_save(&self) -> Result<(), String> {
        if self.connection != Connection::Online || self.state.is_none() {
            return Err("Waiting for k100d.".into());
        }
        if self.recording() {
            return Err("Finish or cancel the recording first".into());
        }
        if self.renaming() {
            return Err("Wait for the rename to finish.".into());
        }
        let Some(draft) = self.draft.as_ref() else { return Err("No profile is loaded yet.".into()) };
        if draft.saving {
            return Err("Saving...".into());
        }
        if !draft.dirty() {
            return Err("Nothing to apply.".into());
        }
        if self.stale.is_some() {
            return Err("Keep your edits or reload first.".into());
        }
        validate_profile(&draft.profile)
    }

    pub fn apply_draft(&mut self) -> bool {
        if self.can_save().is_err() {
            return false;
        }
        let Some(draft) = self.draft.as_mut() else { return false };
        draft.saving = true;
        let profile = draft.profile.clone();
        self.send(Request::SaveProfile { profile });
        true
    }

    pub fn allow_close(&mut self) -> bool {
        let unsaved = self.draft.as_ref().is_some_and(Draft::dirty);
        if close_allowed(self.closing, unsaved) {
            return true;
        }
        if self.closing == Closing::Open {
            self.closing = Closing::Asking;
        }
        false
    }

    pub fn save_and_close(&mut self) {
        if self.draft.as_ref().is_some_and(Draft::refused) {
            return;
        }
        if self.apply_draft() {
            self.closing = Closing::SavingThenClose;
        }
    }

    pub fn close_without_saving(&mut self) {
        self.closing = Closing::Confirmed;
        self.effects.push(Effect::CloseWindow);
    }

    pub fn cancel_close(&mut self) {
        self.closing = Closing::Open;
    }

    fn save_ended(&mut self, saved: bool) {
        if self.closing != Closing::SavingThenClose {
            return;
        }
        if saved && !self.draft.as_ref().is_some_and(Draft::dirty) {
            self.close_without_saving();
        } else {
            self.closing = Closing::Asking;
        }
    }

    pub fn keep_edits(&mut self) {
        self.stale = None;
    }

    fn replace_draft(&mut self, draft: Option<Draft>) {
        let unchanged = matches!((&self.draft, &draft), (Some(old), Some(new)) if old.profile == new.profile);
        if !unchanged {
            self.name_edit = None;
        }
        self.draft = draft;
        self.stale = None;
        self.pending_macro = None;
    }

    pub fn discard_draft(&mut self) {
        self.replace_draft(None);
        let active = self.active_profile().to_owned();
        if !active.is_empty() {
            self.send(Request::GetProfile { name: active });
        }
    }

    pub fn tick(&mut self, now: Duration) {
        let Some(draft) = self.draft.as_mut() else { return };
        if draft.profile != draft.seen {
            draft.seen = draft.profile.clone();
            draft.edited_at = now;
        }
        let quiet = now.saturating_sub(draft.edited_at) >= AUTO_APPLY_QUIET;
        let refused = draft.refused();
        if self.auto_apply && quiet && !refused && self.can_save().is_ok() {
            self.apply_draft();
        }
    }

    pub fn apply(&mut self, inbox: Inbox) {
        match inbox {
            Inbox::Connected => {
                self.connection = Connection::Online;
                self.starting_daemon = false;
                match self.draft.as_mut().filter(|draft| draft.dirty()) {
                    Some(draft) => {
                        draft.rejected = None;
                        let name = draft.profile.name.clone();
                        self.send(Request::GetProfile { name });
                    }
                    None => self.replace_draft(None),
                }
                self.send(Request::GetState);
                self.send(Request::GetSettings);
            }
            Inbox::Disconnected(reason) => {
                self.connection = Connection::Offline(reason);
                self.state = None;
                self.frame = None;
                self.pending_rename = None;
                if let Some(draft) = self.draft.as_mut() {
                    draft.saving = false;
                }
                self.save_ended(false);
            }
            Inbox::LaunchFailed(message) => {
                self.starting_daemon = false;
                self.toast(message);
            }
            Inbox::LaunchFinished => self.starting_daemon = false,
            Inbox::RequestFailed(request, message) => self.request_failed(*request, message),
            Inbox::Reply(pair) => {
                let (request, reply) = *pair;
                self.reply(request, reply);
            }
            Inbox::Event(event) => self.event(event),
        }
    }

    fn drafted(&mut self, name: &str) -> Option<&mut Draft> {
        self.draft.as_mut().filter(|draft| draft.profile.name == name)
    }

    fn request_failed(&mut self, request: Request, message: String) {
        self.toast(message);
        match request {
            Request::SaveProfile { profile } => {
                if self.pending_rename.as_ref().is_some_and(|pending| pending.new == profile.name) {
                    self.pending_rename = None;
                } else if let Some(draft) = self.drafted(&profile.name) {
                    draft.saving = false;
                    self.save_ended(false);
                }
            }
            Request::SetBrightness { .. } => self.device_form.brightness = None,
            _ => {}
        }
    }

    fn reply(&mut self, request: Request, reply: Reply) {
        if self.settle_rename(&request, &reply) {
            return;
        }
        match (request, reply) {
            (Request::SaveProfile { profile }, Reply::Error { message }) => {
                if let Some(draft) = self.drafted(&profile.name) {
                    draft.saving = false;
                    draft.rejected = Some(profile);
                    self.save_ended(false);
                }
                self.toast(message);
            }
            (Request::SetBrightness { .. }, Reply::Error { message }) => {
                self.device_form.brightness = None;
                self.toast(message);
                self.send(Request::GetState);
            }
            (_, Reply::Error { message }) => self.toast(message),
            (Request::GetState, Reply::State { state }) => self.accept_state(state),
            (Request::GetSettings, Reply::Settings { settings }) => self.settings = Some(settings),
            (Request::GetProfile { .. }, Reply::Profile { profile }) => self.accept_profile(profile),
            (Request::SaveProfile { profile }, Reply::Done) => self.accept_saved(profile),
            (Request::DeleteProfile { .. }, Reply::Done) => self.send(Request::GetState),
            (
                Request::SetPollingRate { .. }
                | Request::SetDebounce { .. }
                | Request::SetSettings { .. }
                | Request::SetAutostart { .. },
                Reply::Done,
            ) => {
                self.send(Request::GetState);
                self.send(Request::GetSettings);
            }
            _ => {}
        }
    }

    fn accept_state(&mut self, state: DaemonState) {
        let active = state.active_profile.clone();
        self.state = Some(state);
        if self.draft.is_none() && !active.is_empty() {
            self.send(Request::GetProfile { name: active });
        }
    }

    fn accept_profile(&mut self, profile: Profile) {
        let Some(draft) = self.draft.as_mut().filter(|draft| draft.dirty()) else {
            self.replace_draft(Some(Draft::new(profile)));
            return;
        };
        if draft.profile.name != profile.name {
            return;
        }
        let recorded = self.pending_macro.take();
        if draft.profile == profile {
            draft.saved = profile;
            self.stale = None;
        } else if let Some(key) = recorded {
            if without_key(&draft.saved, &key) == without_key(&profile, &key) {
                take_assignment(&mut draft.profile, &profile, &key);
                draft.saved = profile;
                self.stale = None;
            } else {
                self.stale = Some(Stale::macro_conflict(&key, &profile.name));
            }
        } else if draft.saved != profile {
            self.stale.get_or_insert_with(|| Stale::changed_outside(&profile.name));
        }
    }

    fn accept_saved(&mut self, profile: Profile) {
        if let Some(draft) = self.drafted(&profile.name) {
            draft.saved = profile;
            draft.saving = false;
            draft.rejected = None;
            self.save_ended(true);
        }
        if self.draft.as_ref().is_some_and(|draft| !draft.dirty()) {
            self.stale = None;
        }
        self.send(Request::GetState);
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Frame { rgb } => self.frame = ipc::decode_frame(&rgb),
            Event::ProfileChanged { name } => {
                if let Some(state) = self.state.as_mut() {
                    state.active_profile = name.clone();
                }
                self.send(Request::GetState);
                if self.draft.as_ref().is_none_or(|draft| !draft.dirty()) {
                    self.send(Request::GetProfile { name });
                }
            }
            Event::ProfileEdited { name } => self.send(Request::GetProfile { name }),
            Event::MacroRecorded { key } => {
                self.toast(format!("Macro recorded into {key}"));
                let active = self.active_profile().to_owned();
                match self.draft.as_ref().filter(|draft| draft.dirty()) {
                    Some(draft) if draft.profile.name != active => {}
                    Some(_) => {
                        self.pending_macro = Some(key);
                        self.send(Request::GetProfile { name: active });
                    }
                    None => self.send(Request::GetProfile { name: active }),
                }
            }
            Event::DeviceState { state } => self.update_state(|current| current.device = state),
            Event::WinLock { enabled } => self.update_state(|current| current.winlock = Some(enabled)),
            Event::DialMode { name } => self.update_state(|current| current.dial_mode = name),
            Event::Recording { state } => self.update_state(|current| current.recording = state),
            Event::Brightness { percent } => self.update_state(|current| current.brightness = percent),
            Event::Key { .. } | Event::Dial { .. } => {}
        }
    }

    fn update_state(&mut self, change: impl FnOnce(&mut DaemonState)) {
        if let Some(state) = self.state.as_mut() {
            change(state);
        }
    }
}

pub fn device_text(device: &DeviceState) -> String {
    match device {
        DeviceState::Connected { model, firmware } => format!("{model} connected (firmware {firmware})"),
        DeviceState::Disconnected => "No keyboard found".to_owned(),
        DeviceState::Blocked { processes } => format!("Blocked by {}", processes.join(", ")),
        DeviceState::Released => "Released to hardware mode".to_owned(),
        DeviceState::Paused { reason } => format!("Paused: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use engine::{EffectConfig, LayerConfig, NamedZone, Zone};
    use k100::Rgb;

    use super::*;
    use crate::testing::{online, profile, sent, state};

    fn edit(model: &mut Model) {
        model.draft.as_mut().unwrap().profile.apps.push("game.exe".into());
    }

    fn get_profile(name: &str) -> Request {
        Request::GetProfile { name: name.into() }
    }

    #[test]
    fn connecting_loads_state_settings_and_then_the_active_profile() {
        let mut model = Model::new();
        assert_eq!(model.connection, Connection::Connecting);
        model.apply(Inbox::Connected);
        assert_eq!(model.connection, Connection::Online);
        assert_eq!(sent(&mut model), [Request::GetState, Request::GetSettings]);
        model.apply(Inbox::reply(Request::GetState, Reply::State { state: state("games", &["default", "games"]) }));
        assert_eq!(sent(&mut model), [get_profile("games")]);
        model.apply(Inbox::reply(get_profile("games"), Reply::Profile { profile: profile("games") }));
        let draft = model.draft.as_ref().unwrap();
        assert_eq!(draft.profile.name, "games");
        assert!(!draft.dirty());
    }

    #[test]
    fn reconnecting_reloads_a_clean_draft_but_keeps_unsaved_edits() {
        let mut clean = online("default", &["default"]);
        clean.apply(Inbox::Disconnected("gone".into()));
        clean.apply(Inbox::Connected);
        assert!(clean.draft.is_none());
        clean.apply(Inbox::reply(Request::GetState, Reply::State { state: state("games", &["default", "games"]) }));
        assert!(sent(&mut clean).contains(&get_profile("games")));

        let mut dirty = online("default", &["default"]);
        edit(&mut dirty);
        dirty.apply(Inbox::Disconnected("gone".into()));
        dirty.apply(Inbox::Connected);
        assert!(dirty.draft.as_ref().unwrap().dirty());
    }

    #[test]
    fn losing_the_daemon_goes_offline_and_keeps_the_unsaved_draft() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        model.frame = Some([Rgb::BLACK; k100::LED_COUNT]);
        model.apply(Inbox::Disconnected("refused".into()));
        assert_eq!(model.connection, Connection::Offline("refused".into()));
        assert!(model.state.is_none() && model.frame.is_none());
        assert!(model.draft.as_ref().unwrap().dirty());
    }

    #[test]
    fn starting_the_daemon_is_requested_only_once_until_it_fails_or_connects() {
        let mut model = Model::new();
        model.apply(Inbox::Disconnected("refused".into()));
        model.request_start_daemon();
        model.request_start_daemon();
        assert_eq!(model.take_effects(), [Effect::StartDaemon]);
        assert!(model.starting_daemon);
        model.apply(Inbox::LaunchFailed("cannot find k100d".into()));
        assert!(!model.starting_daemon);
        assert_eq!(model.toasts, ["cannot find k100d"]);
        model.request_start_daemon();
        assert_eq!(model.take_effects(), [Effect::StartDaemon]);
        model.apply(Inbox::Connected);
        assert!(!model.starting_daemon);
    }

    #[test]
    fn the_start_button_comes_back_when_the_launcher_finishes_without_a_connection() {
        let mut model = Model::new();
        model.apply(Inbox::Disconnected("refused".into()));
        model.request_start_daemon();
        model.take_effects();
        model.apply(Inbox::LaunchFinished);
        assert!(!model.starting_daemon);
        model.request_start_daemon();
        assert_eq!(model.take_effects(), [Effect::StartDaemon]);
    }

    #[test]
    fn a_refused_brightness_write_reverts_the_shown_value_and_refetches_state() {
        let mut model = online("default", &["default"]);
        crate::device::commit_brightness(&mut model, 30);
        assert_eq!(model.state.as_ref().unwrap().brightness, 30);
        model.take_effects();
        let request = Request::SetBrightness { percent: 30 };
        model.device_form.brightness = Some(30);
        model.apply(Inbox::reply(request, Reply::Error { message: "settings.toml does not parse".into() }));
        assert_eq!(model.toasts, ["settings.toml does not parse"]);
        assert_eq!(model.device_form.brightness, None);
        assert_eq!(sent(&mut model), [Request::GetState]);
        model.apply(Inbox::reply(Request::GetState, Reply::State { state: state("default", &["default"]) }));
        assert_eq!(model.state.as_ref().unwrap().brightness, 80);
    }

    #[test]
    fn errors_become_toasts_and_only_the_newest_are_kept() {
        let mut model = online("default", &["default"]);
        for number in 0..6 {
            model.apply(Inbox::reply(Request::Release, Reply::Error { message: format!("problem {number}") }));
        }
        assert_eq!(model.toasts, ["problem 2", "problem 3", "problem 4", "problem 5"]);
    }

    #[test]
    fn a_clean_draft_follows_the_active_profile_but_a_dirty_one_stays() {
        let mut model = online("default", &["default", "games"]);
        model.apply(Inbox::Event(Event::ProfileChanged { name: "games".into() }));
        assert_eq!(sent(&mut model), [Request::GetState, get_profile("games")]);
        assert_eq!(model.active_profile(), "games");

        let mut dirty = online("default", &["default", "games"]);
        edit(&mut dirty);
        dirty.apply(Inbox::Event(Event::ProfileChanged { name: "games".into() }));
        assert_eq!(sent(&mut dirty), [Request::GetState]);
        assert_eq!(dirty.draft.as_ref().unwrap().profile.name, "default");
    }

    fn with_macro(mut profile: Profile, key: &str) -> Profile {
        let action = actions::Action::Text { text: format!("recorded into {key}") };
        let key = actions::Key::named(key).unwrap();
        profile.assignments.retain(|assignment| assignment.key != key);
        profile.assignments.push(actions::Assignment { key, action, pass_through: vec![] });
        profile
    }

    #[test]
    fn a_recorded_macro_reloads_a_clean_draft() {
        let mut clean = online("default", &["default"]);
        clean.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        assert_eq!(sent(&mut clean), [get_profile("default")]);
        assert!(clean.toasts[0].contains("g1"));
        let recorded = with_macro(profile("default"), "g1");
        clean.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: recorded.clone() }));
        assert_eq!(clean.draft.as_ref().unwrap().profile, recorded);
    }

    #[test]
    fn a_macro_recorded_under_unsaved_edits_is_merged_into_the_draft() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        model.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        assert_eq!(sent(&mut model), [get_profile("default")]);
        assert!(model.stale.is_none());
        let recorded = with_macro(profile("default"), "g1");
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: recorded.clone() }));
        assert!(model.stale.is_none());
        let draft = model.draft.as_ref().unwrap();
        assert_eq!(draft.saved, recorded);
        assert_eq!(draft.profile.apps, ["game.exe"]);
        assert_eq!(draft.profile.assignments, recorded.assignments);
        let requests = auto_save(&mut model, Duration::from_secs(1));
        let [Request::SaveProfile { profile: saved }] = requests.as_slice() else { panic!("expected a save") };
        assert_eq!(
            (saved.apps.as_slice(), &saved.assignments),
            (["game.exe".to_owned()].as_slice(), &recorded.assignments)
        );
    }

    #[test]
    fn a_macro_merge_replaces_only_the_recorded_key() {
        let mut model = online("default", &["default"]);
        let before = with_macro(with_macro(profile("default"), "g1"), "g2");
        model.draft = Some(Draft::new(before.clone()));
        edit(&mut model);
        model.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        sent(&mut model);
        let mut recorded = before.clone();
        recorded.assignments[0].action = actions::Action::Text { text: "again".into() };
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: recorded.clone() }));
        assert!(model.stale.is_none());
        let draft = model.draft.as_ref().unwrap();
        assert_eq!(draft.profile.assignments, recorded.assignments);
        assert_eq!(draft.profile.apps, ["game.exe"]);
    }

    #[test]
    fn the_edit_notice_of_a_recorded_macro_does_not_leave_the_banner_behind() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        let recorded = with_macro(profile("default"), "g1");
        model.apply(Inbox::Event(Event::ProfileEdited { name: "default".into() }));
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: recorded.clone() }));
        assert!(model.stale.is_some());
        model.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        assert_eq!(sent(&mut model), [get_profile("default"), get_profile("default")]);
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: recorded.clone() }));
        assert!(model.stale.is_none());
        assert_eq!(model.draft.as_ref().unwrap().profile.assignments, recorded.assignments);
    }

    #[test]
    fn a_macro_and_an_outside_edit_together_fall_back_to_the_banner_and_hold_saves() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        model.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        sent(&mut model);
        let mut recorded = with_macro(profile("default"), "g1");
        recorded.apps.push("other.exe".into());
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: recorded }));
        let stale = model.stale.clone().unwrap();
        assert!(stale.drops_macro && stale.message.contains("g1"));
        assert_eq!(model.draft.as_ref().unwrap().profile, changed_apps("default", &["game.exe"]));
        assert!(model.can_save().is_err());
        assert!(!model.apply_draft());
        assert!(auto_save(&mut model, Duration::from_secs(1)).is_empty());
        model.keep_edits();
        assert!(model.apply_draft());
    }

    #[test]
    fn a_macro_recorded_into_another_profile_leaves_a_dirty_draft_alone() {
        let mut model = online("default", &["default", "games"]);
        model.state.as_mut().unwrap().active_profile = "games".into();
        edit(&mut model);
        model.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        assert!(model.stale.is_none());
        assert!(model.toasts[0].contains("g1"));
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: profile("default") }));
        assert_eq!(model.draft.as_ref().unwrap().profile.apps, ["game.exe"]);
    }

    #[test]
    fn a_changed_profile_under_a_dirty_draft_marks_it_stale_instead_of_replacing_it() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        let mut changed = profile("default");
        changed.apps.push("other.exe".into());
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: changed }));
        assert!(model.stale.is_some());
        assert_eq!(model.draft.as_ref().unwrap().profile.apps, ["game.exe"]);
        model.keep_edits();
        assert!(model.stale.is_none());
    }

    #[test]
    fn an_external_edit_silently_reloads_a_clean_draft() {
        let mut model = online("default", &["default"]);
        model.apply(Inbox::Event(Event::ProfileEdited { name: "default".into() }));
        assert_eq!(sent(&mut model), [get_profile("default")]);
        let mut changed = profile("default");
        changed.apps.push("other.exe".into());
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: changed.clone() }));
        let draft = model.draft.as_ref().unwrap();
        assert_eq!(draft.profile, changed);
        assert!(!draft.dirty() && model.stale.is_none());
    }

    #[test]
    fn an_external_edit_under_a_dirty_draft_shows_the_stale_banner() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        model.apply(Inbox::Event(Event::ProfileEdited { name: "default".into() }));
        assert_eq!(sent(&mut model), [get_profile("default")]);
        let mut changed = profile("default");
        changed.apps.push("other.exe".into());
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: changed }));
        assert!(model.stale.as_ref().unwrap().message.contains("default"));
        assert_eq!(model.draft.as_ref().unwrap().profile.apps, ["game.exe"]);
    }

    #[test]
    fn the_echo_of_our_own_save_does_not_raise_the_stale_banner() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        let saved = model.draft.as_ref().unwrap().profile.clone();
        model.apply(Inbox::Event(Event::ProfileEdited { name: "default".into() }));
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: saved }));
        assert!(model.stale.is_none());
        assert!(!model.draft.as_ref().unwrap().dirty());
    }

    #[test]
    fn a_macro_notice_is_not_replaced_by_the_generic_stale_message() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        model.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        let mut changed = profile("default");
        changed.apps.push("other.exe".into());
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: changed }));
        assert!(model.stale.as_ref().unwrap().message.contains("g1"));
    }

    #[test]
    fn apply_sends_the_draft_once_and_marks_it_saved_when_the_daemon_confirms() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        assert!(model.apply_draft());
        let requests = sent(&mut model);
        let [Request::SaveProfile { profile: sent_profile }] = requests.as_slice() else {
            panic!("expected one save");
        };
        let sent_profile = sent_profile.clone();
        assert!(!model.apply_draft(), "a save is already in flight");
        assert!(sent(&mut model).is_empty());
        model.apply(Inbox::reply(Request::SaveProfile { profile: sent_profile }, Reply::Done));
        let draft = model.draft.as_ref().unwrap();
        assert!(!draft.dirty() && !draft.saving);
        assert_eq!(sent(&mut model), [Request::GetState]);
    }

    #[test]
    fn invalid_drafts_are_never_sent() {
        let mut model = online("default", &["default"]);
        let layer = || LayerConfig::new("same", Zone::Named(NamedZone::All), EffectConfig::Solid { color: Rgb::BLACK });
        model.draft.as_mut().unwrap().profile.layers = vec![layer(), layer()];
        assert!(model.draft_problem().unwrap().contains("two layers"));
        assert!(!model.apply_draft());
        model.tick(Duration::ZERO);
        model.tick(Duration::from_secs(10));
        assert!(model.take_effects().is_empty());
    }

    #[test]
    fn auto_apply_waits_for_a_quiet_moment() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        model.tick(Duration::from_millis(1000));
        assert!(model.take_effects().is_empty(), "the edit was just noticed");
        model.draft.as_mut().unwrap().profile.apps.push("again.exe".into());
        model.tick(Duration::from_millis(1300));
        assert!(model.take_effects().is_empty(), "another edit restarted the wait");
        model.tick(Duration::from_millis(1700));
        assert!(matches!(sent(&mut model).as_slice(), [Request::SaveProfile { .. }]));
        model.tick(Duration::from_millis(3000));
        assert!(model.take_effects().is_empty(), "already saving");
    }

    #[test]
    fn nothing_is_saved_while_the_recorder_is_not_idle() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        model.state.as_mut().unwrap().recording = RecorderState::AwaitingKey;
        edit(&mut model);
        model.tick(Duration::from_secs(1));
        model.tick(Duration::from_secs(10));
        assert!(model.take_effects().is_empty(), "the quiet period passed but the recorder is waiting");
        assert!(!model.apply_draft());
        assert!(model.take_effects().is_empty());
        assert!(!model.draft.as_ref().unwrap().saving);
        model.state.as_mut().unwrap().recording = RecorderState::Recording { key: 131 };
        model.tick(Duration::from_secs(20));
        assert!(model.take_effects().is_empty(), "still recording");
        model.state.as_mut().unwrap().recording = RecorderState::Idle;
        model.tick(Duration::from_secs(21));
        assert!(matches!(sent(&mut model).as_slice(), [Request::SaveProfile { .. }]));
    }

    #[test]
    fn a_manual_apply_goes_out_once_the_recorder_is_idle() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        model.state.as_mut().unwrap().recording = RecorderState::AwaitingKey;
        assert!(!model.apply_draft());
        model.state.as_mut().unwrap().recording = RecorderState::Idle;
        assert!(model.apply_draft());
        assert!(matches!(sent(&mut model).as_slice(), [Request::SaveProfile { .. }]));
    }

    #[test]
    fn auto_apply_can_be_switched_off() {
        let mut model = online("default", &["default"]);
        model.auto_apply = false;
        model.tick(Duration::ZERO);
        edit(&mut model);
        model.tick(Duration::from_secs(1));
        model.tick(Duration::from_secs(5));
        assert!(model.take_effects().is_empty());
    }

    #[test]
    fn a_refused_save_is_reported_once_and_not_retried_until_the_draft_changes() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        model.tick(Duration::from_secs(1));
        model.tick(Duration::from_secs(2));
        let Some(Request::SaveProfile { profile }) = sent(&mut model).pop() else { panic!("expected a save") };
        model.apply(Inbox::reply(
            Request::SaveProfile { profile },
            Reply::Error { message: "lighting trigger names the unknown layer \"x\"".into() },
        ));
        assert_eq!(model.toasts.len(), 1);
        model.tick(Duration::from_secs(10));
        model.tick(Duration::from_secs(20));
        assert!(model.take_effects().is_empty());
        model.draft.as_mut().unwrap().profile.apps.push("fixed.exe".into());
        model.tick(Duration::from_secs(21));
        model.tick(Duration::from_secs(22));
        assert!(matches!(sent(&mut model).as_slice(), [Request::SaveProfile { .. }]));
    }

    #[test]
    fn reverting_discards_the_draft_and_reloads_it() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        model.stale = Some(Stale::changed_outside("default"));
        model.discard_draft();
        assert!(model.draft.is_none() && model.stale.is_none());
        assert_eq!(sent(&mut model), [get_profile("default")]);
    }

    #[test]
    fn events_keep_the_state_current() {
        let mut model = online("default", &["default"]);
        model.apply(Inbox::Event(Event::Brightness { percent: 40 }));
        model.apply(Inbox::Event(Event::WinLock { enabled: true }));
        model.apply(Inbox::Event(Event::DialMode { name: "zoom".into() }));
        model.apply(Inbox::Event(Event::Recording { state: actions::RecorderState::AwaitingKey }));
        model.apply(Inbox::Event(Event::DeviceState { state: DeviceState::Released }));
        let state = model.state.as_ref().unwrap();
        assert_eq!(state.brightness, 40);
        assert_eq!(state.winlock, Some(true));
        assert_eq!(state.dial_mode, "zoom");
        assert_eq!(state.recording, actions::RecorderState::AwaitingKey);
        assert_eq!(state.device, DeviceState::Released);
        let mut frame = [Rgb::BLACK; k100::LED_COUNT];
        frame[26] = Rgb::new(255, 0, 0);
        model.apply(Inbox::Event(Event::Frame { rgb: ipc::encode_frame(&frame) }));
        assert_eq!(model.frame.unwrap()[26], Rgb::new(255, 0, 0));
        model.apply(Inbox::Event(Event::Frame { rgb: "garbage".into() }));
        assert!(model.frame.is_none());
    }

    #[test]
    fn settings_changes_refresh_the_state_and_settings() {
        let mut model = online("default", &["default"]);
        model.apply(Inbox::reply(Request::SetPollingRate { hz: 1000 }, Reply::Done));
        assert_eq!(sent(&mut model), [Request::GetState, Request::GetSettings]);
        model.apply(Inbox::reply(
            Request::GetSettings,
            Reply::Settings { settings: Settings { fps: 30, ..Settings::default() } },
        ));
        assert_eq!(model.settings.unwrap().fps, 30);
    }

    #[test]
    fn auto_apply_is_held_back_while_the_draft_is_stale() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        model.stale = Some(Stale::changed_outside("default"));
        model.tick(Duration::from_secs(1));
        model.tick(Duration::from_secs(5));
        assert!(model.take_effects().is_empty());
    }

    fn changed_apps(name: &str, apps: &[&str]) -> Profile {
        let mut changed = profile(name);
        changed.apps = apps.iter().map(|app| (*app).to_owned()).collect();
        changed
    }

    fn reconnect(model: &mut Model, on_disk: Profile) {
        model.apply(Inbox::Disconnected("gone".into()));
        model.apply(Inbox::Connected);
        let name = on_disk.name.clone();
        assert_eq!(sent(model), [get_profile(&name), Request::GetState, Request::GetSettings]);
        model.apply(Inbox::reply(get_profile(&name), Reply::Profile { profile: on_disk }));
        model.apply(Inbox::reply(Request::GetState, Reply::State { state: state(&name, &[&name]) }));
    }

    fn auto_save(model: &mut Model, from: Duration) -> Vec<Request> {
        model.tick(from);
        model.tick(from + AUTO_APPLY_QUIET * 2);
        sent(model)
    }

    #[test]
    fn a_lost_connection_during_a_save_is_retried_after_reconnecting() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        let requests = auto_save(&mut model, Duration::from_secs(1));
        let [save @ Request::SaveProfile { .. }] = requests.as_slice() else { panic!("expected a save: {requests:?}") };
        let save = save.clone();
        model.apply(Inbox::request_failed(save, "cannot reach k100d"));
        assert_eq!(model.toasts, ["cannot reach k100d"]);
        let draft = model.draft.as_ref().unwrap();
        assert!(!draft.saving && draft.rejected.is_none());
        reconnect(&mut model, profile("default"));
        assert!(model.stale.is_none());
        assert!(matches!(auto_save(&mut model, Duration::from_secs(10)).as_slice(), [Request::SaveProfile { .. }]));
    }

    #[test]
    fn a_profile_changed_on_disk_while_offline_raises_the_banner_after_reconnecting() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        reconnect(&mut model, changed_apps("default", &["other.exe"]));
        assert!(model.stale.is_some());
        assert_eq!(model.draft.as_ref().unwrap().profile.apps, ["game.exe"]);
        assert!(auto_save(&mut model, Duration::from_secs(10)).is_empty());
    }

    #[test]
    fn reconnecting_clears_an_earlier_refusal() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        let draft = model.draft.as_mut().unwrap();
        draft.rejected = Some(draft.profile.clone());
        reconnect(&mut model, profile("default"));
        assert!(model.draft.as_ref().unwrap().rejected.is_none());
        assert!(matches!(auto_save(&mut model, Duration::from_secs(10)).as_slice(), [Request::SaveProfile { .. }]));
    }

    #[test]
    fn nothing_is_saved_while_offline_or_before_the_state_arrives() {
        let mut model = online("default", &["default"]);
        model.tick(Duration::ZERO);
        edit(&mut model);
        model.apply(Inbox::Disconnected("gone".into()));
        assert!(auto_save(&mut model, Duration::from_secs(1)).is_empty());
        assert!(!model.apply_draft());
        model.apply(Inbox::Connected);
        sent(&mut model);
        assert!(auto_save(&mut model, Duration::from_secs(5)).is_empty(), "the state has not arrived yet");
        assert!(!model.apply_draft());
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: profile("default") }));
        model.apply(Inbox::reply(Request::GetState, Reply::State { state: state("default", &["default"]) }));
        assert!(matches!(auto_save(&mut model, Duration::from_secs(10)).as_slice(), [Request::SaveProfile { .. }]));
    }

    #[test]
    fn a_refused_or_lost_save_of_another_profile_leaves_the_draft_alone() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        assert!(model.apply_draft());
        sent(&mut model);
        model.create_profile("work").unwrap();
        let [create] = sent(&mut model).try_into().unwrap();
        model.apply(Inbox::reply(create.clone(), Reply::Error { message: "disk full".into() }));
        model.apply(Inbox::request_failed(create.clone(), "cannot reach k100d"));
        model.apply(Inbox::reply(create, Reply::Done));
        let draft = model.draft.as_ref().unwrap();
        assert!(draft.saving && draft.rejected.is_none() && draft.dirty());
    }

    #[test]
    fn a_lost_brightness_write_reverts_the_shown_value() {
        let mut model = online("default", &["default"]);
        model.device_form.brightness = Some(30);
        model.apply(Inbox::request_failed(Request::SetBrightness { percent: 30 }, "cannot reach k100d"));
        assert_eq!(model.device_form.brightness, None);
        assert_eq!(model.toasts, ["cannot reach k100d"]);
    }

    #[test]
    fn a_draft_that_matches_the_disk_again_drops_the_banner() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        let mine = model.draft.as_ref().unwrap().profile.clone();
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: changed_apps("default", &["x"]) }));
        assert!(model.stale.is_some());
        model.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: mine }));
        assert!(model.stale.is_none());
        model.tick(Duration::ZERO);
        edit(&mut model);
        assert!(matches!(auto_save(&mut model, Duration::from_secs(1)).as_slice(), [Request::SaveProfile { .. }]));
    }

    #[test]
    fn a_buffered_layer_name_is_dropped_whenever_the_draft_is_dropped() {
        let typing = || Some(("glow".to_owned(), "glowx".to_owned()));
        let mut reconnected = online("default", &["default"]);
        reconnected.name_edit = typing();
        reconnected.apply(Inbox::Disconnected("gone".into()));
        reconnected.apply(Inbox::Connected);
        assert!(reconnected.name_edit.is_none());

        let mut reverted = online("default", &["default"]);
        reverted.name_edit = typing();
        reverted.discard_draft();
        assert!(reverted.name_edit.is_none());

        let mut switched = online("default", &["default", "games"]);
        switched.name_edit = typing();
        switched.edit_profile("games").unwrap();
        assert!(switched.name_edit.is_none());

        let mut echoed = online("default", &["default"]);
        echoed.name_edit = typing();
        echoed.apply(Inbox::reply(get_profile("default"), Reply::Profile { profile: profile("default") }));
        assert_eq!(echoed.name_edit, typing(), "an unchanged reload keeps what is being typed");
    }

    fn closes(model: &mut Model) -> bool {
        model.take_effects().contains(&Effect::CloseWindow)
    }

    #[test]
    fn the_window_only_asks_before_closing_over_unsaved_edits() {
        for closing in [Closing::Open, Closing::Asking, Closing::SavingThenClose, Closing::Confirmed] {
            assert!(close_allowed(closing, false), "{closing:?}");
        }
        assert!(!close_allowed(Closing::Open, true));
        assert!(!close_allowed(Closing::Asking, true));
        assert!(!close_allowed(Closing::SavingThenClose, true));
        assert!(close_allowed(Closing::Confirmed, true));
    }

    #[test]
    fn closing_a_clean_window_needs_no_prompt() {
        let mut model = online("default", &["default"]);
        assert!(model.allow_close());
        assert_eq!(model.closing, Closing::Open);
    }

    #[test]
    fn closing_over_unsaved_edits_asks_and_cancel_stays_open() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        assert!(!model.allow_close());
        assert_eq!(model.closing, Closing::Asking);
        model.cancel_close();
        assert_eq!(model.closing, Closing::Open);
        assert!(!closes(&mut model));
    }

    #[test]
    fn save_and_close_closes_once_the_daemon_confirms_the_save() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        assert!(!model.allow_close());
        model.save_and_close();
        assert_eq!(model.closing, Closing::SavingThenClose);
        let requests = sent(&mut model);
        let [save @ Request::SaveProfile { .. }] = requests.as_slice() else { panic!("expected a save: {requests:?}") };
        assert!(!model.allow_close(), "still saving");
        model.apply(Inbox::reply(save.clone(), Reply::Done));
        assert_eq!(model.closing, Closing::Confirmed);
        assert!(closes(&mut model));
        assert!(model.allow_close());
    }

    #[test]
    fn a_save_that_fails_while_closing_asks_again() {
        let mut refused = online("default", &["default"]);
        edit(&mut refused);
        refused.allow_close();
        refused.save_and_close();
        let [save] = sent(&mut refused).try_into().unwrap();
        refused.apply(Inbox::reply(save, Reply::Error { message: "disk full".into() }));
        assert_eq!(refused.closing, Closing::Asking);
        assert!(!closes(&mut refused));

        let mut lost = online("default", &["default"]);
        edit(&mut lost);
        lost.allow_close();
        lost.save_and_close();
        let [save] = sent(&mut lost).try_into().unwrap();
        lost.apply(Inbox::request_failed(save, "cannot reach k100d"));
        assert_eq!(lost.closing, Closing::Asking);

        let mut dropped = online("default", &["default"]);
        edit(&mut dropped);
        dropped.allow_close();
        dropped.save_and_close();
        dropped.apply(Inbox::Disconnected("gone".into()));
        assert_eq!(dropped.closing, Closing::Asking);
        assert!(!closes(&mut dropped));
    }

    #[test]
    fn save_and_close_does_nothing_when_the_draft_cannot_be_saved() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        model.allow_close();
        model.stale = Some(Stale::changed_outside("default"));
        model.save_and_close();
        assert_eq!(model.closing, Closing::Asking);
        assert!(model.take_effects().is_empty());

        model.stale = None;
        let draft = model.draft.as_mut().unwrap();
        draft.rejected = Some(draft.profile.clone());
        model.save_and_close();
        assert_eq!(model.closing, Closing::Asking);
        assert!(model.take_effects().is_empty(), "the daemon already refused exactly these edits");
    }

    #[test]
    fn closing_without_saving_closes_right_away() {
        let mut model = online("default", &["default"]);
        edit(&mut model);
        model.allow_close();
        model.close_without_saving();
        assert!(closes(&mut model));
        assert!(model.allow_close());
    }

    #[test]
    fn items_move_until_the_edge() {
        let mut items = ["a", "b", "c"];
        assert_eq!(move_item(&mut items, 0, true), Some(1));
        assert_eq!(items, ["b", "a", "c"]);
        assert_eq!(move_item(&mut items, 2, true), None);
        assert_eq!(move_item(&mut items, 0, false), None);
        assert_eq!(move_item(&mut items, 2, false), Some(1));
        assert_eq!(items, ["b", "c", "a"]);
        assert_eq!(move_item(&mut items, 7, false), None);
        assert_eq!(move_item(&mut items, 7, true), None);
    }

    #[test]
    fn profile_validation_names_the_problem() {
        let mut bad = profile("p");
        bad.name = "  ".into();
        assert!(validate_profile(&bad).unwrap_err().contains("needs a name"));
        let mut empty_zone = profile("p");
        empty_zone.layers = vec![LayerConfig::new("a", Zone::Leds(vec![]), EffectConfig::Solid { color: Rgb::BLACK })];
        assert!(validate_profile(&empty_zone).unwrap_err().contains("empty zone"));
        let mut unnamed = profile("p");
        unnamed.layers =
            vec![LayerConfig::new("", Zone::Named(NamedZone::All), EffectConfig::Solid { color: Rgb::BLACK })];
        assert!(validate_profile(&unnamed).unwrap_err().contains("needs a name"));
        assert!(validate_profile(&profile("fine")).is_ok());
    }
}
