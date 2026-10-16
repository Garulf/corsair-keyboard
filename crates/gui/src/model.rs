use std::collections::BTreeSet;
use std::time::Duration;

use engine::{Layout, Zone};
use ipc::{DaemonState, DeviceState, Event, Profile, Reply, Request, Settings};
use k100::{Frame, Rgb};

use crate::canvas::CanvasState;

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
}

impl Pane {
    pub const ALL: &[Pane] = &[Pane::Overview, Pane::Lighting, Pane::Keys];

    pub fn label(self) -> &'static str {
        match self {
            Pane::Overview => "Overview",
            Pane::Lighting => "Lighting",
            Pane::Keys => "Keys",
        }
    }

    pub fn edits_profile(self) -> bool {
        match self {
            Pane::Overview => false,
            Pane::Lighting => true,
            Pane::Keys => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inbox {
    Connected,
    Disconnected(String),
    LaunchFailed(String),
    Reply(Box<(Request, Reply)>),
    Event(Event),
}

impl Inbox {
    pub fn reply(request: Request, reply: Reply) -> Self {
        Self::Reply(Box::new((request, reply)))
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
    pub stale: Option<String>,
    pub frame: Option<Frame>,
    pub layout: Layout,
    pub selection: BTreeSet<u8>,
    pub pane: Pane,
    pub toasts: Vec<String>,
    pub starting_daemon: bool,
    pub auto_apply: bool,
    pub canvas: CanvasState,
    pub layer: usize,
    pub name_edit: Option<(String, String)>,
    pub paint: Rgb,
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
            name_edit: None,
            paint: Rgb::new(255, 255, 255),
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

    pub fn apply_draft(&mut self) -> bool {
        if self.draft_problem().is_some() {
            return false;
        }
        let Some(draft) = self.draft.as_mut() else { return false };
        if draft.saving || !draft.dirty() {
            return false;
        }
        draft.saving = true;
        let profile = draft.profile.clone();
        self.send(Request::SaveProfile { profile });
        true
    }

    pub fn keep_edits(&mut self) {
        self.stale = None;
    }

    pub fn discard_draft(&mut self) {
        self.draft = None;
        self.stale = None;
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
        let refused = draft.rejected.as_ref() == Some(&draft.profile);
        let due = self.auto_apply && draft.dirty() && !draft.saving && quiet && !refused && self.stale.is_none();
        if due {
            self.apply_draft();
        }
    }

    pub fn apply(&mut self, inbox: Inbox) {
        match inbox {
            Inbox::Connected => {
                self.connection = Connection::Online;
                self.starting_daemon = false;
                if self.draft.as_ref().is_none_or(|draft| !draft.dirty()) {
                    self.draft = None;
                    self.stale = None;
                }
                self.send(Request::GetState);
                self.send(Request::GetSettings);
            }
            Inbox::Disconnected(reason) => {
                self.connection = Connection::Offline(reason);
                self.state = None;
                self.frame = None;
                if let Some(draft) = self.draft.as_mut() {
                    draft.saving = false;
                }
            }
            Inbox::LaunchFailed(message) => {
                self.starting_daemon = false;
                self.toast(message);
            }
            Inbox::Reply(pair) => {
                let (request, reply) = *pair;
                self.reply(request, reply);
            }
            Inbox::Event(event) => self.event(event),
        }
    }

    fn reply(&mut self, request: Request, reply: Reply) {
        match (request, reply) {
            (Request::SaveProfile { profile }, Reply::Error { message }) => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.saving = false;
                    draft.rejected = Some(profile);
                }
                self.toast(message);
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
            self.draft = Some(Draft::new(profile));
            self.stale = None;
            return;
        };
        if draft.profile.name != profile.name {
            return;
        }
        if draft.profile == profile {
            draft.saved = profile;
        } else if draft.saved != profile {
            self.stale.get_or_insert_with(|| format!("{} changed outside this window.", profile.name));
        }
    }

    fn accept_saved(&mut self, profile: Profile) {
        if let Some(draft) = self.draft.as_mut()
            && draft.profile.name == profile.name
        {
            draft.saved = profile;
            draft.saving = false;
            draft.rejected = None;
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
                if self.draft.as_ref().is_some_and(Draft::dirty) {
                    self.stale = Some(format!("A macro was recorded into {key} while you had unsaved edits."));
                } else {
                    let active = self.active_profile().to_owned();
                    self.send(Request::GetProfile { name: active });
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

    #[test]
    fn a_recorded_macro_never_overwrites_unsaved_edits_silently() {
        let mut clean = online("default", &["default"]);
        clean.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        assert_eq!(sent(&mut clean), [get_profile("default")]);
        assert!(clean.toasts[0].contains("g1"));

        let mut dirty = online("default", &["default"]);
        edit(&mut dirty);
        dirty.apply(Inbox::Event(Event::MacroRecorded { key: "g1".into() }));
        assert!(sent(&mut dirty).is_empty());
        assert!(dirty.stale.as_deref().unwrap().contains("g1"));
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
        assert!(model.stale.as_deref().unwrap().contains("default"));
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
        assert!(model.stale.as_deref().unwrap().contains("g1"));
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
        model.stale = Some("changed".into());
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
        model.stale = Some("changed".into());
        model.tick(Duration::from_secs(1));
        model.tick(Duration::from_secs(5));
        assert!(model.take_effects().is_empty());
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
