use std::collections::BTreeMap;
use std::time::Duration;

use actions::{
    Action, Assignment, BrightnessChange, Dispatcher, Intent, Key, MacroStep, OutputCommand, ProfileTarget,
    RecorderState, RepeatMode,
};
use bragi::{PollingRate, PropertyId};
use engine::{Compositor, KeyPress, Layout};
use ipc::{
    DaemonState, Debounce, DeviceState, Event, MAX_DEBOUNCE_TIMEOUT, PROTOCOL_VERSION, Profile, Reply, Request,
    SettingsPatch,
};
use k100::{DeviceError, Frame, IndicatorRecord, InputEvent, KeyMappings, Session, Transport, led};

use crate::config::{ConfigDir, Settings, default_profile, profile_file_name};
use crate::indicators::{DialRing, Indicators};

const BRIGHTNESS_SCALE: u32 = 10;
const BRIGHTNESS_STEP: u8 = 4;
const BRIGHTNESS_SAVE_DELAY: Duration = Duration::from_secs(1);
const WINLOCK_SETTLE: Duration = Duration::from_millis(150);

pub trait OutputPort: Send {
    fn send(&mut self, command: OutputCommand);
    fn is_busy(&self) -> bool;
}

impl OutputPort for actions::OutputHandle {
    fn send(&mut self, command: OutputCommand) {
        actions::OutputHandle::send(self, command);
    }

    fn is_busy(&self) -> bool {
        actions::OutputHandle::is_busy(self)
    }
}

pub struct DiscardOutput;

impl OutputPort for DiscardOutput {
    fn send(&mut self, _command: OutputCommand) {}

    fn is_busy(&self) -> bool {
        false
    }
}

pub trait DeviceLink: Send {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), DeviceError>;
    fn write_indicators(&mut self, records: &[IndicatorRecord]) -> Result<(), DeviceError>;
    fn set_key_mappings(&mut self, mappings: &KeyMappings) -> Result<(), DeviceError>;
    fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError>;
    fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError>;
    fn try_set_property(&mut self, property: PropertyId, value: u32) -> Result<bool, DeviceError>;
    fn key_count(&self) -> usize;
    fn ping(&mut self) -> Result<(), DeviceError>;
    fn release(&mut self) -> Result<(), DeviceError>;
}

impl<T: Transport> DeviceLink for Session<T> {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), DeviceError> {
        Session::write_frame(self, frame)
    }

    fn write_indicators(&mut self, records: &[IndicatorRecord]) -> Result<(), DeviceError> {
        Session::write_indicators(self, records)
    }

    fn set_key_mappings(&mut self, mappings: &KeyMappings) -> Result<(), DeviceError> {
        Session::set_key_mappings(self, mappings)
    }

    fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError> {
        Session::get_property(self, property)
    }

    fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
        Session::set_property(self, property, value)
    }

    fn try_set_property(&mut self, property: PropertyId, value: u32) -> Result<bool, DeviceError> {
        Session::try_set_property(self, property, value)
    }

    fn key_count(&self) -> usize {
        self.model().key_count
    }

    fn ping(&mut self) -> Result<(), DeviceError> {
        Session::ping(self)
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        Session::release(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub model: String,
    pub firmware: String,
    pub max_polling_rate: Option<PollingRate>,
}

pub struct Controller {
    settings: Settings,
    profiles: BTreeMap<String, Profile>,
    active: String,
    applied: Profile,
    chosen: Option<String>,
    foreground: Option<String>,
    manual_override: bool,
    focus_pending: bool,
    compositor: Compositor,
    dispatcher: Dispatcher,
    output: Box<dyn OutputPort>,
    output_available: bool,
    indicators: Indicators,
    link: Option<Box<dyn DeviceLink>>,
    max_polling_rate: Option<PollingRate>,
    device: DeviceState,
    last_ping: Duration,
    profile_errors: Vec<String>,
    settings_error: Option<String>,
    device_warning: Option<String>,
    debounce_warning: Option<String>,
    warnings: Vec<String>,
    winlock: Option<bool>,
    winlock_check: Option<Duration>,
    brightness_changed_at: Option<Duration>,
    now: Duration,
    events: Vec<Event>,
    last_frame: Option<Frame>,
    autostart: Box<dyn install::Autostart>,
}

impl Controller {
    pub fn new(
        settings: Settings,
        profiles: Vec<Profile>,
        config_errors: Vec<String>,
        output: Box<dyn OutputPort>,
    ) -> Self {
        let mut profiles: BTreeMap<String, Profile> = profiles.into_iter().map(|p| (p.name.clone(), p)).collect();
        if profiles.is_empty() {
            let fallback = default_profile();
            profiles.insert(fallback.name.clone(), fallback);
        }
        let active = pick_active(&profiles, None, &settings.default_profile);
        let profile = &profiles[&active];
        let compositor = Compositor::new(Layout::ansi(), &profile.layers);
        let dispatcher = Dispatcher::new(&profile.assignments, &profile.dial);
        let indicators = Indicators::new(settings.dial_ring);
        let applied = profile.clone();
        Self {
            settings,
            profiles,
            active,
            applied,
            chosen: None,
            foreground: None,
            manual_override: false,
            focus_pending: false,
            compositor,
            dispatcher,
            output,
            output_available: true,
            indicators,
            link: None,
            max_polling_rate: None,
            device: DeviceState::Disconnected,
            last_ping: Duration::ZERO,
            profile_errors: config_errors,
            settings_error: None,
            device_warning: None,
            debounce_warning: None,
            warnings: Vec::new(),
            winlock: None,
            winlock_check: None,
            brightness_changed_at: None,
            now: Duration::ZERO,
            events: Vec::new(),
            last_frame: None,
            autostart: Box::new(install::Unavailable("autostart is not configured".into())),
        }
    }

    pub fn set_autostart(&mut self, autostart: Box<dyn install::Autostart>) {
        self.autostart = autostart;
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    pub fn set_output_available(&mut self, available: bool) {
        self.output_available = available;
        self.write_key_mappings();
    }

    pub fn state(&self) -> DaemonState {
        let mut config_errors: Vec<String> = self.settings_error.iter().cloned().collect();
        config_errors.extend(self.profile_errors.iter().cloned());
        config_errors.extend(self.device_warning.iter().cloned());
        config_errors.extend(self.debounce_warning.iter().cloned());
        config_errors.extend(self.warnings.iter().cloned());
        DaemonState {
            device: self.device.clone(),
            active_profile: self.active.clone(),
            profiles: self.profiles.keys().cloned().collect(),
            brightness: self.settings.brightness.min(100),
            polling_rate_hz: self.settings.polling_rate_hz,
            max_polling_rate_hz: self.max_polling_rate.map(PollingRate::hz),
            config_errors,
            winlock: self.winlock,
            dial_mode: self.dispatcher.dial_mode().label(),
            recording: self.dispatcher.recorder_state(),
            autostart: self.autostart.is_enabled().unwrap_or(false),
        }
    }

    pub fn wants_device(&self) -> bool {
        self.link.is_none() && self.device != DeviceState::Released
    }

    pub fn last_frame(&self) -> Option<Frame> {
        self.last_frame
    }

    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs(1) / self.settings.fps.clamp(1, 200)
    }

    pub fn ignore_conflicts(&self) -> &[String] {
        &self.settings.ignore_conflicts
    }

    pub fn attach(&mut self, link: Box<dyn DeviceLink>, info: LinkInfo, now: Duration) {
        self.link = Some(link);
        self.max_polling_rate = info.max_polling_rate;
        self.last_ping = now;
        self.device_warning = None;
        let _ = self.apply_brightness();
        if let Some(hz) = self.settings.polling_rate_hz
            && let Err(message) = self.apply_polling_rate(hz)
        {
            self.device_warning = Some(message);
        }
        self.debounce_warning = self.settings.debounce.and_then(|debounce| self.apply_debounce(debounce).err());
        self.indicators.reset();
        self.show_continuous_dial();
        self.write_key_mappings();
        if let Some(enabled) = self.read_winlock() {
            self.set_winlock_state(enabled, false);
        }
        self.apply_winlock_shortcuts();
        if self.link.is_some() {
            self.set_state(DeviceState::Connected { model: info.model, firmware: info.firmware });
        }
    }

    pub fn set_device_state(&mut self, state: DeviceState) {
        self.drop_link();
        self.set_state(state);
    }

    pub fn input(&mut self, event: InputEvent, now: Duration, config: &ConfigDir) {
        self.now = now;
        match event {
            InputEvent::Property { property, value } if property == PropertyId::WIN_LOCK_ENABLED => {
                self.winlock_check = None;
                self.set_winlock_state(value != 0, true);
            }
            InputEvent::KeyDown(led::WIN_LOCK) => self.winlock_check = Some(now + WINLOCK_SETTLE),
            _ => {}
        }
        let intents = self.dispatcher.handle(&event, now);
        self.apply_all(intents, config);
    }

    pub fn tick(&mut self, now: Duration, presses: &[KeyPress], config: &ConfigDir) {
        self.now = now;
        let intents = self.dispatcher.poll(now);
        self.apply_all(intents, config);
        if self.focus_pending && !self.output_busy() {
            self.apply_focus();
        }
        if self.winlock_check.is_some_and(|due| now >= due) {
            self.winlock_check = None;
            self.resolve_winlock();
        }
        self.save_brightness_when_settled(config);
        if self.link.is_none() {
            return;
        }
        let frame = self.compositor.render(now, presses);
        self.last_frame = Some(frame);
        let _ = self.with_link(|link| link.write_frame(&frame));
        let records = self.indicators.records(now);
        if !records.is_empty() {
            let _ = self.with_link(|link| link.write_indicators(&records));
        }
        if now.saturating_sub(self.last_ping) >= k100::PING_INTERVAL {
            self.last_ping = now;
            let _ = self.with_link(|link| link.ping());
        }
    }

    pub fn handle(&mut self, request: Request, config: &ConfigDir) -> Reply {
        match request {
            Request::Hello { .. } => Reply::Hello { version: PROTOCOL_VERSION },
            Request::GetState => Reply::State { state: self.state() },
            Request::ListProfiles => Reply::Profiles { names: self.profiles.keys().cloned().collect() },
            Request::GetProfile { name } => match self.profiles.get(&name) {
                Some(profile) => Reply::Profile { profile: profile.clone() },
                None => error(format!("no profile named {name:?}")),
            },
            Request::SaveProfile { profile } => self.save_profile(profile, config),
            Request::DeleteProfile { name } => self.delete_profile(&name, config),
            Request::SetActiveProfile { name } => {
                if !self.profiles.contains_key(&name) {
                    return error(format!("no profile named {name:?}"));
                }
                self.chosen = Some(name.clone());
                self.manual_override = true;
                self.activate(name);
                Reply::Done
            }
            Request::SetBrightness { percent } => self.set_brightness(percent, config),
            Request::SetPollingRate { hz } => self.set_polling_rate(hz, config),
            Request::SetDebounce { enabled, timeout } => self.set_debounce(Debounce { enabled, timeout }, config),
            Request::GetSettings => Reply::Settings { settings: self.settings.clone() },
            Request::SetSettings { patch } => self.set_settings(patch, config),
            Request::StartRecording { key } => self.start_recording(key),
            Request::StopRecording => {
                let intents = self.dispatcher.stop_recording();
                self.apply_all(intents, config);
                Reply::Done
            }
            Request::CancelRecording => {
                self.cancel_recording();
                Reply::Done
            }
            Request::SetAutostart { enabled } => {
                let result = if enabled { self.autostart.enable() } else { self.autostart.disable() };
                match result {
                    Ok(()) => Reply::Done,
                    Err(failure) => error(format!("autostart: {failure}")),
                }
            }
            Request::SetWinLock { enabled } => {
                if self.link.is_none() {
                    return error("no keyboard connected".into());
                }
                match self.write_winlock(enabled) {
                    Ok(()) => Reply::Done,
                    Err(failure) => error(format!("keyboard rejected WinLock: {failure}")),
                }
            }
            Request::Release => self.release(),
            Request::Reacquire => {
                if self.device == DeviceState::Released {
                    self.set_state(DeviceState::Disconnected);
                }
                Reply::Done
            }
            Request::Subscribe | Request::SubscribeFrames => Reply::Done,
        }
    }

    pub fn reload(&mut self, profiles: Vec<Profile>, config_errors: Vec<String>, failed_files: Vec<String>) {
        self.profile_errors = config_errors;
        let mut fresh: BTreeMap<String, Profile> = profiles.into_iter().map(|p| (p.name.clone(), p)).collect();
        for (name, profile) in &self.profiles {
            if failed_files.contains(&profile_file_name(name)) && !fresh.contains_key(name) {
                fresh.insert(name.clone(), profile.clone());
            }
        }
        if fresh.is_empty() {
            return;
        }
        self.profiles = fresh;
        let active = self.preferred_profile();
        self.activate(active);
    }

    pub fn reload_settings(&mut self, settings: Result<Settings, String>) {
        match settings {
            Ok(mut settings) => {
                if self.brightness_changed_at.is_some() {
                    settings.brightness = self.settings.brightness;
                }
                self.settings_error = None;
                let polling_changed = settings.polling_rate_hz != self.settings.polling_rate_hz;
                let brightness_changed = settings.brightness != self.settings.brightness;
                let shortcuts_changed = settings.winlock_disables != self.settings.winlock_disables;
                let debounce_changed = settings.debounce != self.settings.debounce;
                let reselect = settings.auto_switch != self.settings.auto_switch
                    || settings.default_profile != self.settings.default_profile;
                self.indicators.set_ring(settings.dial_ring);
                self.settings = settings;
                if brightness_changed {
                    let _ = self.apply_brightness();
                }
                if let (true, Some(hz)) = (polling_changed, self.settings.polling_rate_hz) {
                    self.device_warning = self.apply_polling_rate(hz).err();
                }
                if shortcuts_changed {
                    self.apply_winlock_shortcuts();
                }
                if debounce_changed {
                    self.debounce_warning =
                        self.settings.debounce.and_then(|debounce| self.apply_debounce(debounce).err());
                }
                if reselect {
                    self.reselect_profile();
                }
            }
            Err(message) => self.settings_error = Some(message),
        }
    }

    pub fn focus(&mut self, exe: Option<String>) {
        if exe != self.foreground {
            self.manual_override = false;
        }
        self.foreground = exe;
        if !self.settings.auto_switch {
            return;
        }
        if self.output_busy() {
            self.focus_pending = true;
        } else {
            self.apply_focus();
        }
    }

    fn output_busy(&self) -> bool {
        self.dispatcher.is_busy() || self.output.is_busy()
    }

    fn reselect_profile(&mut self) {
        if self.output_busy() {
            self.focus_pending = true;
        } else {
            self.apply_focus();
        }
    }

    fn apply_focus(&mut self) {
        self.focus_pending = false;
        let target = self.preferred_profile();
        self.activate(target);
    }

    fn preferred_profile(&self) -> String {
        let focused = self
            .foreground
            .as_deref()
            .filter(|_| self.settings.auto_switch && !self.manual_override)
            .and_then(|exe| crate::focus::profile_for(self.profiles.values(), exe));
        pick_active(&self.profiles, focused.as_deref().or(self.chosen.as_deref()), &self.settings.default_profile)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn shutdown(&mut self) {
        self.output.send(OutputCommand::CancelAll);
        self.dispatcher.forget_held_keys();
        if let Some(mut link) = self.link.take() {
            let _ = link.release();
        }
    }

    fn active_profile(&self) -> &Profile {
        &self.profiles[&self.active]
    }

    fn activate(&mut self, name: String) {
        let changed = name != self.active;
        self.active = name;
        let profile = self.active_profile().clone();
        if !changed && profile == self.applied {
            return;
        }
        self.output.send(OutputCommand::CancelAll);
        self.compositor = Compositor::new(Layout::ansi(), &profile.layers);
        self.dispatcher.reconfigure(&profile.assignments, &profile.dial);
        self.applied = profile;
        if changed {
            self.cancel_recording();
            self.indicators.profile_switched(self.now);
            self.events.push(Event::ProfileChanged { name: self.active.clone() });
        } else {
            self.events.push(Event::ProfileEdited { name: self.active.clone() });
        }
        self.show_continuous_dial();
        self.write_key_mappings();
    }

    fn show_continuous_dial(&mut self) {
        if self.settings.dial_ring == DialRing::Continuous {
            let color = self.dispatcher.dial_mode().color();
            self.indicators.dial_mode(color, self.now);
        }
    }

    fn cancel_recording(&mut self) {
        if self.dispatcher.cancel_recording() {
            self.indicators.recording(RecorderState::Idle);
            self.events.push(Event::Recording { state: RecorderState::Idle });
        }
    }

    fn drop_link(&mut self) {
        self.last_frame = None;
        if self.link.take().is_some() {
            self.output.send(OutputCommand::CancelAll);
            self.dispatcher.forget_held_keys();
            self.cancel_recording();
            self.winlock = None;
            self.winlock_check = None;
        }
    }

    fn apply_all(&mut self, intents: Vec<Intent>, config: &ConfigDir) {
        for intent in intents {
            self.apply(intent, config);
        }
    }

    fn apply(&mut self, intent: Intent, config: &ConfigDir) {
        match intent {
            Intent::Output(command) => self.output.send(command),
            Intent::SwitchProfile(target) => {
                if let Some(name) = self.resolve_target(target) {
                    self.chosen = Some(name.clone());
                    self.manual_override = true;
                    self.activate(name);
                }
            }
            Intent::Brightness(change) => self.step_brightness(change),
            Intent::LightingTrigger { layer, on: true, duration_ms } => {
                let until = duration_ms.map(|ms| self.now + Duration::from_millis(u64::from(ms)));
                self.compositor.trigger_layer(&layer, until);
            }
            Intent::LightingTrigger { layer, on: false, .. } => self.compositor.release_layer(&layer),
            Intent::DialMode { .. } => {
                let mode = self.dispatcher.dial_mode();
                let (color, name) = (mode.color(), mode.label());
                self.indicators.dial_mode(color, self.now);
                self.events.push(Event::DialMode { name });
            }
            Intent::Recording(state) => {
                self.indicators.recording(state);
                self.events.push(Event::Recording { state });
            }
            Intent::SaveMacro { key, steps } => self.save_macro(key, steps, config),
        }
    }

    fn resolve_target(&self, target: ProfileTarget) -> Option<String> {
        let names: Vec<&String> = self.profiles.keys().collect();
        let position = names.iter().position(|name| **name == self.active)?;
        let count = names.len();
        match target {
            ProfileTarget::Named(name) => self.profiles.contains_key(&name).then_some(name),
            ProfileTarget::Next => Some(names[(position + 1) % count].clone()),
            ProfileTarget::Previous => Some(names[(position + count - 1) % count].clone()),
        }
    }

    fn step_brightness(&mut self, change: BrightnessChange) {
        let current = self.settings.brightness.min(100);
        let target = match change {
            BrightnessChange::Up => (current / BRIGHTNESS_STEP + 1) * BRIGHTNESS_STEP,
            BrightnessChange::Down => current.saturating_sub(1) / BRIGHTNESS_STEP * BRIGHTNESS_STEP,
            BrightnessChange::Set(percent) => percent,
        }
        .min(100);
        if target == current {
            return;
        }
        self.settings.brightness = target;
        let _ = self.apply_brightness();
        self.brightness_changed_at = Some(self.now);
        self.events.push(Event::Brightness { percent: target });
    }

    fn save_brightness_when_settled(&mut self, config: &ConfigDir) {
        let settled = self.brightness_changed_at.is_some_and(|at| self.now.saturating_sub(at) >= BRIGHTNESS_SAVE_DELAY);
        if !settled || self.settings_error.is_some() {
            return;
        }
        let Ok(mut on_disk) = config.load_settings() else { return };
        self.brightness_changed_at = None;
        on_disk.brightness = self.settings.brightness;
        let _ = config.save_settings(&on_disk);
    }

    fn save_macro(&mut self, key: Key, steps: Vec<MacroStep>, config: &ConfigDir) {
        let mut profile = self.active_profile().clone();
        profile.assignments.retain(|assignment| assignment.key != key);
        let action = Action::Macro { steps, mode: RepeatMode::Once };
        profile.assignments.push(Assignment { key, action, pass_through: Vec::new() });
        match self.save_profile(profile, config) {
            Reply::Error { message } => self.warnings.push(format!("recorded macro not saved: {message}")),
            _ => self.events.push(Event::MacroRecorded { key: key.name().to_owned() }),
        }
    }

    fn read_winlock(&mut self) -> Option<bool> {
        let mut value = None;
        let _ =
            self.with_link(|link| link.get_property(PropertyId::WIN_LOCK_ENABLED).map(|raw| value = Some(raw != 0)));
        value
    }

    fn resolve_winlock(&mut self) {
        let Some(on_device) = self.read_winlock() else { return };
        if Some(on_device) == self.winlock {
            let _ = self.write_winlock(!on_device);
        } else {
            self.set_winlock_state(on_device, true);
        }
    }

    fn write_winlock(&mut self, enabled: bool) -> Result<(), DeviceError> {
        self.with_link(|link| link.set_property(PropertyId::WIN_LOCK_ENABLED, u32::from(enabled)))?;
        self.set_winlock_state(enabled, true);
        Ok(())
    }

    fn set_winlock_state(&mut self, enabled: bool, announce: bool) {
        let changed = self.winlock != Some(enabled);
        self.winlock = Some(enabled);
        self.indicators.winlock(enabled, self.now, announce && changed);
        if changed {
            self.events.push(Event::WinLock { enabled });
        }
    }

    fn apply_winlock_shortcuts(&mut self) {
        let Some(shortcuts) = self.settings.winlock_disables.clone() else { return };
        let mask = shortcuts.iter().fold(0, |mask, shortcut| mask | shortcut.bit());
        let _ = self.with_link(|link| link.try_set_property(PropertyId::WIN_LOCK_DISABLED_SHORTCUTS, mask).map(drop));
    }

    fn write_key_mappings(&mut self) {
        let mut assignments = self.active_profile().assignments.clone();
        if !self.output_available {
            assignments.retain(|assignment| works_without_output(&assignment.action));
        }
        let result = self.with_link(|link| {
            let mappings: KeyMappings = actions::key_mappings(&assignments, link.key_count());
            link.set_key_mappings(&mappings)
        });
        if let Err(failure) = result
            && failure.status().is_some()
        {
            self.device_warning = Some(format!("keyboard rejected the key assignments: {failure}"));
        }
    }

    fn save_profile(&mut self, profile: Profile, config: &ConfigDir) -> Reply {
        if profile.name.trim().is_empty() {
            return error("profile name must not be empty".into());
        }
        if let Err(problem) = profile.validate() {
            return error(problem);
        }
        if let Err(failure) = config.save_profile(&profile) {
            return error(format!("saving profile: {failure}"));
        }
        let name = profile.name.clone();
        self.profiles.insert(name.clone(), profile);
        if name == self.active {
            self.activate(name);
        }
        Reply::Done
    }

    fn delete_profile(&mut self, name: &str, config: &ConfigDir) -> Reply {
        if name == self.active {
            return error("cannot delete the active profile".into());
        }
        if !self.profiles.contains_key(name) {
            return error(format!("no profile named {name:?}"));
        }
        if let Err(failure) = config.delete_profile(name) {
            return error(format!("deleting profile: {failure}"));
        }
        self.profiles.remove(name);
        Reply::Done
    }

    fn set_polling_rate(&mut self, hz: u32, config: &ConfigDir) -> Reply {
        let mut on_disk = match config.load_settings() {
            Ok(settings) => settings,
            Err(message) => return error(message),
        };
        if let Err(message) = self.apply_polling_rate(hz) {
            return error(message);
        }
        on_disk.polling_rate_hz = Some(hz);
        if let Err(failure) = config.save_settings(&on_disk) {
            return error(format!("saving settings: {failure}"));
        }
        self.settings.polling_rate_hz = Some(hz);
        self.device_warning = None;
        Reply::Done
    }

    fn apply_polling_rate(&mut self, hz: u32) -> Result<(), String> {
        let rate = PollingRate::from_hz(hz)
            .ok_or_else(|| format!("unsupported polling rate {hz} Hz (125, 250, 500, 1000, 2000, 4000, 8000)"))?;
        if let Some(max) = self.max_polling_rate.filter(|max| rate > *max) {
            return Err(format!("this keyboard supports at most {} Hz", max.hz()));
        }
        self.with_link(|link| link.set_property(PropertyId::POLLING_RATE, rate.raw()))
            .map_err(|failure| format!("keyboard rejected {hz} Hz: {failure}"))
    }

    fn set_settings(&mut self, patch: SettingsPatch, config: &ConfigDir) -> Reply {
        if let Some(name) = &patch.default_profile
            && !self.profiles.contains_key(name)
        {
            return error(format!("no profile named {name:?}"));
        }
        let mut on_disk = match config.load_settings() {
            Ok(settings) => settings,
            Err(message) => return error(message),
        };
        patch.apply(&mut on_disk);
        if let Err(failure) = config.save_settings(&on_disk) {
            return error(format!("saving settings: {failure}"));
        }
        self.reload_settings(Ok(on_disk));
        Reply::Done
    }

    fn set_debounce(&mut self, debounce: Debounce, config: &ConfigDir) -> Reply {
        let mut on_disk = match config.load_settings() {
            Ok(settings) => settings,
            Err(message) => return error(message),
        };
        if let Err(message) = self.apply_debounce(debounce) {
            return error(message);
        }
        on_disk.debounce = Some(debounce);
        if let Err(failure) = config.save_settings(&on_disk) {
            return error(format!("saving settings: {failure}"));
        }
        self.settings.debounce = Some(debounce);
        self.debounce_warning = None;
        Reply::Done
    }

    fn apply_debounce(&mut self, debounce: Debounce) -> Result<(), String> {
        if debounce.timeout > MAX_DEBOUNCE_TIMEOUT {
            return Err(format!("debounce time must be 0 to {MAX_DEBOUNCE_TIMEOUT}"));
        }
        let enabled = u32::from(debounce.enabled);
        let writes = [
            (PropertyId::PRESS_DEBOUNCE_ENABLED, enabled),
            (PropertyId::PRESS_DEBOUNCE_TIMEOUT, debounce.timeout),
            (PropertyId::RELEASE_DEBOUNCE_ENABLED, enabled),
            (PropertyId::RELEASE_DEBOUNCE_TIMEOUT, debounce.timeout),
        ];
        for (property, value) in writes {
            self.with_link(|link| link.set_property(property, value))
                .map_err(|failure| format!("keyboard rejected the debounce setting: {failure}"))?;
        }
        Ok(())
    }

    fn start_recording(&mut self, key: Option<u8>) -> Reply {
        if self.link.is_none() {
            return error("no keyboard connected".into());
        }
        let state = match key {
            None => self.dispatcher.arm_recording(),
            Some(index) => match self.dispatcher.record_into(index) {
                Ok(state) => state,
                Err(message) => return error(message),
            },
        };
        self.indicators.recording(state);
        self.events.push(Event::Recording { state });
        Reply::Done
    }

    fn release(&mut self) -> Reply {
        self.output.send(OutputCommand::CancelAll);
        self.dispatcher.forget_held_keys();
        self.cancel_recording();
        self.winlock = None;
        self.winlock_check = None;
        self.last_frame = None;
        let result = self.link.take().map_or(Ok(()), |mut link| link.release());
        self.set_state(DeviceState::Released);
        match result {
            Ok(()) => Reply::Done,
            Err(failure) => error(format!("release reported: {failure}")),
        }
    }

    fn set_brightness(&mut self, percent: u8, config: &ConfigDir) -> Reply {
        let mut on_disk = match config.load_settings() {
            Ok(settings) => settings,
            Err(message) => return error(message),
        };
        on_disk.brightness = percent.min(100);
        if let Err(failure) = config.save_settings(&on_disk) {
            return error(format!("saving settings: {failure}"));
        }
        self.settings.brightness = on_disk.brightness;
        self.brightness_changed_at = None;
        match self.apply_brightness() {
            Ok(()) => Reply::Done,
            Err(failure) => error(format!("keyboard rejected the brightness: {failure}")),
        }
    }

    fn apply_brightness(&mut self) -> Result<(), DeviceError> {
        let value = u32::from(self.settings.brightness.min(100)) * BRIGHTNESS_SCALE;
        self.with_link(|link| link.set_property(PropertyId::BRIGHTNESS, value))
    }

    fn with_link(
        &mut self,
        operation: impl FnOnce(&mut dyn DeviceLink) -> Result<(), DeviceError>,
    ) -> Result<(), DeviceError> {
        let Some(link) = self.link.as_deref_mut() else { return Ok(()) };
        let result = operation(link);
        if let Err(failure) = &result
            && failure.status().is_none()
        {
            self.drop_link();
            self.set_state(DeviceState::Paused { reason: failure.to_string() });
        }
        result
    }

    fn set_state(&mut self, state: DeviceState) {
        if self.device != state {
            self.device = state.clone();
            self.events.push(Event::DeviceState { state });
        }
    }
}

fn pick_active(profiles: &BTreeMap<String, Profile>, chosen: Option<&str>, preferred: &str) -> String {
    chosen
        .filter(|name| profiles.contains_key(*name))
        .or_else(|| profiles.contains_key(preferred).then_some(preferred))
        .map(str::to_owned)
        .unwrap_or_else(|| profiles.keys().next().cloned().unwrap_or_default())
}

fn error(message: String) -> Reply {
    Reply::Error { message }
}

fn works_without_output(action: &Action) -> bool {
    matches!(
        action,
        Action::Disable
            | Action::SwitchProfile { .. }
            | Action::NextProfile
            | Action::PreviousProfile
            | Action::LightingTrigger { .. }
            | Action::BrightnessUp
            | Action::BrightnessDown
            | Action::SetBrightness { .. }
    )
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use actions::{Action, Assignment, Key, MacroStep, OutputCommand, RecorderState, RepeatMode};
    use k100::{IndicatorRecord, Rgb, led};

    use super::*;
    use crate::config::default_profile;

    #[derive(Default)]
    struct Log {
        frames: usize,
        last_frame: Option<Frame>,
        properties: Vec<(u16, u32)>,
        property_reads: Vec<u16>,
        winlock_on_device: u32,
        mappings: Vec<Vec<u8>>,
        indicators: Vec<k100::IndicatorRecord>,
        pings: usize,
        released: bool,
        fail_writes: bool,
        reject_polling: bool,
    }

    struct FakeLink(Arc<Mutex<Log>>);

    impl DeviceLink for FakeLink {
        fn write_frame(&mut self, frame: &Frame) -> Result<(), DeviceError> {
            let mut log = self.0.lock().unwrap();
            if log.fail_writes {
                return Err(DeviceError::Timeout(bragi::Command::WriteBufferBegin));
            }
            log.frames += 1;
            log.last_frame = Some(*frame);
            Ok(())
        }
        fn write_indicators(&mut self, records: &[k100::IndicatorRecord]) -> Result<(), DeviceError> {
            self.0.lock().unwrap().indicators.extend_from_slice(records);
            Ok(())
        }
        fn set_key_mappings(&mut self, mappings: &k100::KeyMappings) -> Result<(), DeviceError> {
            self.0.lock().unwrap().mappings.push(mappings.as_bytes().to_vec());
            Ok(())
        }
        fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError> {
            let mut log = self.0.lock().unwrap();
            log.property_reads.push(property.0);
            Ok(if property == PropertyId::WIN_LOCK_ENABLED { log.winlock_on_device } else { 0 })
        }
        fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
            let mut log = self.0.lock().unwrap();
            if log.reject_polling && property == PropertyId::POLLING_RATE {
                let status = bragi::Status::InvalidArgument;
                return Err(DeviceError::Status { command: bragi::Command::SetProperty, status });
            }
            if property == PropertyId::WIN_LOCK_ENABLED {
                log.winlock_on_device = value;
            }
            log.properties.push((property.0, value));
            Ok(())
        }
        fn try_set_property(&mut self, property: PropertyId, value: u32) -> Result<bool, DeviceError> {
            self.set_property(property, value).map(|()| true)
        }
        fn key_count(&self) -> usize {
            138
        }
        fn ping(&mut self) -> Result<(), DeviceError> {
            self.0.lock().unwrap().pings += 1;
            Ok(())
        }
        fn release(&mut self) -> Result<(), DeviceError> {
            self.0.lock().unwrap().released = true;
            Ok(())
        }
    }

    fn info(max_polling_rate: Option<PollingRate>) -> LinkInfo {
        LinkInfo { model: "K100".into(), firmware: "1.15.61".into(), max_polling_rate }
    }

    fn profile(name: &str) -> Profile {
        Profile { name: name.into(), ..default_profile() }
    }

    #[derive(Clone, Default)]
    struct FakeOutput {
        commands: Arc<Mutex<Vec<OutputCommand>>>,
        busy: Arc<AtomicBool>,
    }

    impl FakeOutput {
        fn take(&self) -> Vec<OutputCommand> {
            std::mem::take(&mut *self.commands.lock().unwrap())
        }

        fn set_busy(&self, busy: bool) {
            self.busy.store(busy, Ordering::SeqCst);
        }
    }

    impl OutputPort for FakeOutput {
        fn send(&mut self, command: OutputCommand) {
            self.commands.lock().unwrap().push(command);
        }

        fn is_busy(&self) -> bool {
            self.busy.load(Ordering::SeqCst)
        }
    }

    struct Rig {
        controller: Controller,
        log: Arc<Mutex<Log>>,
        output: FakeOutput,
        _temp: tempfile::TempDir,
        config: ConfigDir,
    }

    fn rig(profiles: Vec<Profile>) -> Rig {
        rig_with(Settings { brightness: 80, ..Settings::default() }, profiles)
    }

    fn rig_with(settings: Settings, profiles: Vec<Profile>) -> Rig {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let output = FakeOutput::default();
        let mut controller = Controller::new(settings, profiles, vec![], Box::new(output.clone()));
        let log = Arc::new(Mutex::new(Log::default()));
        controller.attach(Box::new(FakeLink(log.clone())), info(Some(PollingRate::Hz8000)), Duration::ZERO);
        Rig { controller, log, output, _temp: temp, config }
    }

    fn controller() -> (Controller, Arc<Mutex<Log>>, tempfile::TempDir, ConfigDir) {
        let rig = rig(vec![profile("default"), profile("games")]);
        (rig.controller, rig.log, rig._temp, rig.config)
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn assigned(name: &str, key_name: &str, action: Action) -> Profile {
        Profile { assignments: vec![Assignment { key: key(key_name), action, pass_through: vec![] }], ..profile(name) }
    }

    fn caps_to_escape() -> Profile {
        assigned("default", "caps_lock", Action::Remap { key: key("escape"), modifiers: vec![] })
    }

    fn tap_key(rig: &mut Rig, index: u8, down_at: u64, up_at: u64) {
        rig.controller.input(InputEvent::KeyDown(index), ms(down_at), &rig.config);
        rig.controller.input(InputEvent::KeyUp(index), ms(up_at), &rig.config);
    }

    #[test]
    fn attach_applies_brightness_and_reports_connected() {
        let (mut controller, log, _temp, _config) = controller();
        assert_eq!(log.lock().unwrap().properties, [(0x02, 800)]);
        assert!(matches!(controller.state().device, DeviceState::Connected { .. }));
        assert!(controller.take_events().iter().any(|e| matches!(e, Event::DeviceState { .. })));
    }

    #[test]
    fn ticks_write_frames_and_ping_on_schedule() {
        let (mut controller, log, _temp, config) = controller();
        controller.tick(Duration::from_millis(16), &[], &config);
        controller.tick(Duration::from_secs(10), &[], &config);
        assert_eq!(log.lock().unwrap().pings, 0);
        controller.tick(Duration::from_secs(25), &[], &config);
        let log = log.lock().unwrap();
        assert_eq!(log.frames, 3);
        assert_eq!(log.pings, 1);
    }

    #[test]
    fn write_failure_pauses_and_drops_the_link() {
        let (mut controller, log, _temp, config) = controller();
        log.lock().unwrap().fail_writes = true;
        controller.tick(Duration::from_millis(16), &[], &config);
        assert!(matches!(controller.state().device, DeviceState::Paused { .. }));
        assert!(controller.wants_device());
        log.lock().unwrap().fail_writes = false;
        controller.tick(Duration::from_millis(32), &[], &config);
        assert_eq!(log.lock().unwrap().frames, 0, "no frames after the link is gone");
    }

    #[test]
    fn switching_profiles_keeps_the_device_brightness_and_announces() {
        let (mut controller, log, _temp, config) = controller();
        controller.take_events();
        let writes_before = log.lock().unwrap().properties.len();
        let reply = controller.handle(Request::SetActiveProfile { name: "games".into() }, &config);
        assert_eq!(reply, Reply::Done);
        assert_eq!(controller.state().active_profile, "games");
        assert_eq!(log.lock().unwrap().properties.len(), writes_before, "brightness is not per profile");
        assert_eq!(controller.take_events(), [Event::ProfileChanged { name: "games".into() }]);
        let missing = controller.handle(Request::SetActiveProfile { name: "nope".into() }, &config);
        assert!(matches!(missing, Reply::Error { .. }));
    }

    #[test]
    fn brightness_and_polling_requests_reach_the_device_and_persist() {
        let (mut controller, log, _temp, config) = controller();
        assert_eq!(controller.handle(Request::SetBrightness { percent: 140 }, &config), Reply::Done);
        assert_eq!(log.lock().unwrap().properties.last(), Some(&(0x02, 1000)));
        assert_eq!(controller.state().brightness, 100);
        assert_eq!(config.load_settings().unwrap().brightness, 100);
        assert!(config.load_profiles().0.is_empty(), "brightness changes do not rewrite profiles");
        assert_eq!(controller.handle(Request::SetPollingRate { hz: 4000 }, &config), Reply::Done);
        assert_eq!(log.lock().unwrap().properties.last(), Some(&(0x01, 6)));
        assert_eq!(config.load_settings().unwrap().polling_rate_hz, Some(4000));
        assert!(matches!(controller.handle(Request::SetPollingRate { hz: 300 }, &config), Reply::Error { .. }));
    }

    #[test]
    fn release_stops_reconnects_until_reacquire() {
        let (mut controller, log, _temp, config) = controller();
        assert_eq!(controller.handle(Request::Release, &config), Reply::Done);
        assert!(log.lock().unwrap().released);
        assert_eq!(controller.state().device, DeviceState::Released);
        assert!(!controller.wants_device());
        assert_eq!(controller.handle(Request::Reacquire, &config), Reply::Done);
        assert!(controller.wants_device());
    }

    #[test]
    fn cannot_delete_the_active_profile() {
        let (mut controller, _log, _temp, config) = controller();
        let reply = controller.handle(Request::DeleteProfile { name: "default".into() }, &config);
        assert!(matches!(reply, Reply::Error { .. }));
    }

    #[test]
    fn foreground_apps_pick_their_profile_and_fall_back() {
        let games = Profile { apps: vec!["Game.exe".into()], ..profile("games") };
        let mut rig = rig(vec![profile("default"), games, profile("other")]);
        rig.controller.focus(Some(r"C:\Games\game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "games");
        rig.controller.focus(Some(r"C:\Windows\notepad.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "default");
        rig.controller.handle(Request::SetActiveProfile { name: "other".into() }, &rig.config);
        rig.controller.focus(Some(r"C:\Windows\explorer.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "other", "the manual choice is the fallback");
        rig.controller.focus(Some(r"C:\Games\game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "games");
        rig.controller.focus(None);
        assert_eq!(rig.controller.state().active_profile, "other");
    }

    fn rig_with_game_focused() -> Rig {
        let games = Profile { apps: vec!["game.exe".into()], ..profile("games") };
        let mut rig = rig(vec![profile("default"), games, profile("other")]);
        rig.controller.focus(Some(r"C:\Games\game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "games");
        rig.controller.handle(Request::SetActiveProfile { name: "other".into() }, &rig.config);
        rig
    }

    #[test]
    fn a_manual_choice_survives_a_reload_while_the_app_stays_focused() {
        let mut rig = rig_with_game_focused();
        let games = Profile { apps: vec!["game.exe".into()], ..profile("games") };
        rig.controller.reload(vec![profile("default"), games, profile("other")], vec![], vec![]);
        assert_eq!(rig.controller.state().active_profile, "other");
    }

    #[test]
    fn a_manual_choice_ends_when_the_foreground_app_changes() {
        let mut rig = rig_with_game_focused();
        rig.controller.focus(Some(r"C:\Windows\notepad.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "other");
        rig.controller.focus(Some(r"C:\Games\game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "games");
    }

    #[test]
    fn repeating_the_same_foreground_app_keeps_the_manual_choice() {
        let mut rig = rig_with_game_focused();
        rig.controller.focus(Some(r"C:\Games\game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "other");
    }

    #[test]
    fn auto_switching_can_be_turned_off() {
        let games = Profile { apps: vec!["game.exe".into()], ..profile("games") };
        let mut rig = rig(vec![profile("default"), games]);
        rig.controller.reload_settings(Ok(Settings { auto_switch: false, ..Settings::default() }));
        rig.controller.focus(Some("game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "default");
    }

    #[test]
    fn reload_keeps_the_last_good_version_of_a_broken_file() {
        let (mut controller, _log, _temp, config) = controller();
        controller.handle(Request::SetActiveProfile { name: "games".into() }, &config);
        controller.take_events();
        controller.reload(vec![profile("default")], vec!["games.toml: broken".into()], vec!["games.toml".into()]);
        assert_eq!(controller.state().active_profile, "games");
        assert_eq!(controller.state().config_errors, ["games.toml: broken"]);
        assert!(controller.take_events().is_empty(), "no profile change announced");
    }

    #[test]
    fn the_chosen_profile_returns_when_its_file_comes_back() {
        let (mut controller, _log, _temp, config) = controller();
        controller.handle(Request::SetActiveProfile { name: "games".into() }, &config);
        controller.reload(vec![profile("default")], vec![], vec![]);
        assert_eq!(controller.state().active_profile, "default");
        controller.reload(vec![profile("default"), profile("games")], vec![], vec![]);
        assert_eq!(controller.state().active_profile, "games");
    }

    #[test]
    fn the_last_rendered_frame_is_available_for_publishing() {
        let mut rig = rig(vec![profile("default")]);
        assert!(rig.controller.last_frame().is_none());
        rig.controller.tick(ms(16), &[], &rig.config);
        assert!(rig.controller.last_frame().is_some());
    }

    #[test]
    fn a_lost_or_released_keyboard_leaves_no_stale_frame_behind() {
        let mut lost = rig(vec![profile("default")]);
        lost.controller.tick(ms(16), &[], &lost.config);
        lost.controller.set_device_state(DeviceState::Disconnected);
        assert!(lost.controller.last_frame().is_none());

        let mut released = rig(vec![profile("default")]);
        released.controller.tick(ms(16), &[], &released.config);
        assert_eq!(released.controller.handle(Request::Release, &released.config), Reply::Done);
        assert!(released.controller.last_frame().is_none());
    }

    #[test]
    fn new_without_profiles_uses_the_built_in_default() {
        let controller = Controller::new(Settings::default(), vec![], vec![], Box::new(DiscardOutput));
        assert_eq!(controller.state().active_profile, "default");
        assert_eq!(controller.state().device, DeviceState::Disconnected);
    }

    #[test]
    fn rejected_polling_rates_keep_the_link() {
        let (mut controller, log, _temp, config) = controller();
        log.lock().unwrap().reject_polling = true;
        let reply = controller.handle(Request::SetPollingRate { hz: 2000 }, &config);
        assert!(matches!(reply, Reply::Error { .. }), "{reply:?}");
        assert!(matches!(controller.state().device, DeviceState::Connected { .. }));
        controller.tick(Duration::from_millis(16), &[], &config);
        assert_eq!(log.lock().unwrap().frames, 1);
    }

    #[test]
    fn polling_above_the_keyboard_maximum_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let mut controller =
            Controller::new(Settings::default(), vec![profile("default")], vec![], Box::new(DiscardOutput));
        let log = Arc::new(Mutex::new(Log::default()));
        controller.attach(Box::new(FakeLink(log.clone())), info(Some(PollingRate::Hz1000)), Duration::ZERO);
        let reply = controller.handle(Request::SetPollingRate { hz: 8000 }, &config);
        assert!(matches!(reply, Reply::Error { ref message } if message.contains("1000")), "{reply:?}");
    }

    #[test]
    fn settings_reload_updates_conflicts_and_polling_saves_keep_other_edits() {
        let (mut controller, _log, _temp, config) = controller();
        let edited = Settings { ignore_conflicts: vec!["OpenRGB.exe".into()], ..Settings::default() };
        config.save_settings(&edited).unwrap();
        controller.reload_settings(config.load_settings());
        assert_eq!(controller.ignore_conflicts(), ["OpenRGB.exe"]);
        controller.reload_settings(Err("settings.toml: broken".into()));
        assert_eq!(controller.ignore_conflicts(), ["OpenRGB.exe"], "last good settings kept");
        assert!(controller.state().config_errors.iter().any(|e| e.contains("settings.toml")));
        let manual = Settings { fps: 30, ..edited };
        config.save_settings(&manual).unwrap();
        assert_eq!(controller.handle(Request::SetPollingRate { hz: 500 }, &config), Reply::Done);
        let saved = config.load_settings().unwrap();
        assert_eq!((saved.fps, saved.polling_rate_hz), (30, Some(500)));
    }

    #[test]
    fn a_settings_reload_with_new_brightness_reaches_the_keyboard() {
        let (mut controller, log, _temp, _config) = controller();
        controller.reload_settings(Ok(Settings { brightness: 25, ..Settings::default() }));
        assert_eq!(log.lock().unwrap().properties.last(), Some(&(0x02, 250)));
        assert_eq!(controller.state().brightness, 25);
    }

    #[test]
    fn saving_an_invalid_profile_is_refused() {
        let (mut controller, _log, _temp, config) = controller();
        let mut bad = profile("bad");
        bad.assignments = vec![actions::Assignment {
            key: actions::Key::named("g1").unwrap(),
            action: actions::Action::Remap { key: actions::Key::named("g2").unwrap(), modifiers: vec![] },
            pass_through: vec![],
        }];
        let reply = controller.handle(Request::SaveProfile { profile: bad }, &config);
        assert!(matches!(reply, Reply::Error { ref message } if message.contains("g2")), "{reply:?}");
        assert!(config.load_profiles().0.is_empty());
    }

    #[test]
    fn attach_writes_the_profile_key_mappings() {
        let rig = rig(vec![caps_to_escape()]);
        let log = rig.log.lock().unwrap();
        let bytes = log.mappings.last().unwrap();
        assert_eq!((bytes.len(), bytes[57], bytes[4]), (138, 0x00, 0x01));
    }

    #[test]
    fn assigned_keys_drive_the_output_worker() {
        let mut rig = rig(vec![caps_to_escape()]);
        tap_key(&mut rig, 57, 10, 20);
        assert_eq!(
            rig.output.take(),
            [
                OutputCommand::Press { source: 57, program: actions::Program::remap(key("escape"), &[]) },
                OutputCommand::Release { source: 57 },
            ]
        );
    }

    #[test]
    fn the_profile_key_cycles_profiles_and_cleans_up() {
        let mut rig = rig(vec![profile("default"), profile("games")]);
        let writes = rig.log.lock().unwrap().mappings.len();
        rig.controller.input(InputEvent::KeyDown(led::PROFILE), ms(10), &rig.config);
        assert_eq!(rig.controller.state().active_profile, "games");
        assert_eq!(rig.output.take(), [OutputCommand::CancelAll]);
        assert_eq!(rig.log.lock().unwrap().mappings.len(), writes + 1);
        rig.controller.tick(ms(20), &[], &rig.config);
        let white = IndicatorRecord { led: led::PROFILE, color: Rgb::new(255, 255, 255), alpha: 0xFF };
        assert!(rig.log.lock().unwrap().indicators.contains(&white));
        rig.controller.tick(ms(3100), &[], &rig.config);
        let handed_back = IndicatorRecord { led: led::PROFILE, color: Rgb::BLACK, alpha: 0 };
        assert_eq!(rig.log.lock().unwrap().indicators.last(), Some(&handed_back));
    }

    #[test]
    fn dial_brightness_steps_and_is_saved_once_it_settles() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.input(InputEvent::DialCw, ms(0), &rig.config);
        rig.controller.input(InputEvent::DialCw, ms(100), &rig.config);
        assert_eq!(rig.log.lock().unwrap().properties[1..], [(0x02, 840), (0x02, 880)]);
        assert_eq!(rig.controller.state().brightness, 88);
        rig.controller.tick(ms(1050), &[], &rig.config);
        assert!(!rig.config.root().join("settings.toml").exists(), "not saved while the dial still moves");
        rig.controller.tick(ms(1100), &[], &rig.config);
        assert_eq!(rig.config.load_settings().unwrap().brightness, 88);
    }

    #[test]
    fn brightness_steps_stop_at_the_limits() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.handle(Request::SetBrightness { percent: 98 }, &rig.config);
        rig.controller.input(InputEvent::DialCw, ms(0), &rig.config);
        rig.controller.input(InputEvent::DialCw, ms(10), &rig.config);
        rig.controller.handle(Request::SetBrightness { percent: 2 }, &rig.config);
        rig.controller.input(InputEvent::DialCcw, ms(20), &rig.config);
        rig.controller.input(InputEvent::DialCcw, ms(30), &rig.config);
        let log = rig.log.lock().unwrap();
        let brightness: Vec<u32> = log.properties.iter().filter(|(p, _)| *p == 0x02).map(|(_, v)| *v).collect();
        assert_eq!(brightness, [800, 980, 1000, 20, 0]);
    }

    #[test]
    fn winlock_follows_the_firmware_when_it_toggles_itself() {
        let mut rig = rig(vec![profile("default")]);
        assert_eq!(rig.controller.state().winlock, Some(false));
        rig.log.lock().unwrap().winlock_on_device = 1;
        rig.controller.input(InputEvent::KeyDown(led::WIN_LOCK), ms(0), &rig.config);
        rig.controller.tick(ms(100), &[], &rig.config);
        assert_eq!(rig.controller.state().winlock, Some(false), "waits for the firmware to settle");
        rig.controller.tick(ms(200), &[], &rig.config);
        assert_eq!(rig.controller.state().winlock, Some(true));
        assert!(!rig.log.lock().unwrap().properties.iter().any(|(p, _)| *p == 0x45), "no second toggle");
        assert!(rig.controller.take_events().contains(&Event::WinLock { enabled: true }));
    }

    #[test]
    fn winlock_is_toggled_by_the_daemon_when_the_firmware_does_not() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.input(InputEvent::KeyDown(led::WIN_LOCK), ms(0), &rig.config);
        rig.controller.tick(ms(200), &[], &rig.config);
        assert!(rig.log.lock().unwrap().properties.contains(&(0x45, 1)));
        assert_eq!(rig.controller.state().winlock, Some(true));
        let red = IndicatorRecord { led: led::WIN_LOCK, color: Rgb::new(255, 0, 0), alpha: 0xFF };
        assert!(rig.log.lock().unwrap().indicators.contains(&red));
    }

    #[test]
    fn a_winlock_notification_settles_the_press_without_reading() {
        let mut rig = rig(vec![profile("default")]);
        let reads = rig.log.lock().unwrap().property_reads.len();
        rig.controller.input(InputEvent::KeyDown(led::WIN_LOCK), ms(0), &rig.config);
        let change = InputEvent::Property { property: PropertyId::WIN_LOCK_ENABLED, value: 1 };
        rig.controller.input(change, ms(50), &rig.config);
        rig.controller.tick(ms(300), &[], &rig.config);
        assert_eq!(rig.log.lock().unwrap().property_reads.len(), reads);
        assert_eq!(rig.controller.state().winlock, Some(true));
    }

    #[test]
    fn set_winlock_requests_reach_the_keyboard() {
        let mut rig = rig(vec![profile("default")]);
        assert_eq!(rig.controller.handle(Request::SetWinLock { enabled: true }, &rig.config), Reply::Done);
        assert!(rig.log.lock().unwrap().properties.contains(&(0x45, 1)));
        assert_eq!(rig.controller.state().winlock, Some(true));
        let mut detached = Controller::new(Settings::default(), vec![], vec![], Box::new(DiscardOutput));
        let reply = detached.handle(Request::SetWinLock { enabled: true }, &rig.config);
        assert!(matches!(reply, Reply::Error { .. }));
    }

    #[test]
    fn recording_a_macro_saves_it_into_the_active_profile() {
        let mut rig = rig(vec![profile("default")]);
        tap_key(&mut rig, led::DIAL_KEY, 0, 50);
        rig.controller.input(InputEvent::KeyDown(led::DIAL_KEY), ms(100), &rig.config);
        rig.controller.tick(ms(700), &[], &rig.config);
        rig.controller.input(InputEvent::KeyUp(led::DIAL_KEY), ms(750), &rig.config);
        assert_eq!(rig.controller.state().recording, RecorderState::AwaitingKey);
        tap_key(&mut rig, led::G1, 800, 850);
        tap_key(&mut rig, 4, 900, 960);
        rig.controller.input(InputEvent::KeyDown(led::G1), ms(1000), &rig.config);
        let steps = vec![MacroStep::Down(key("a")), MacroStep::Delay(60), MacroStep::Up(key("a"))];
        let expected = Assignment {
            key: key("g1"),
            action: Action::Macro { steps, mode: RepeatMode::Once },
            pass_through: vec![],
        };
        assert_eq!(rig.config.load_profiles().0[0].assignments, [expected]);
        assert!(rig.controller.take_events().contains(&Event::MacroRecorded { key: "g1".into() }));
        assert_eq!(rig.log.lock().unwrap().mappings.last().unwrap()[131], 0x00);
    }

    #[test]
    fn releasing_the_keyboard_lets_go_of_held_keys() {
        let mut rig = rig(vec![caps_to_escape()]);
        rig.controller.input(InputEvent::KeyDown(57), ms(0), &rig.config);
        rig.output.take();
        rig.controller.handle(Request::Release, &rig.config);
        assert_eq!(rig.output.take(), [OutputCommand::CancelAll]);
    }

    #[test]
    fn a_dropped_link_forgets_keys_whose_release_was_lost() {
        let mut profile = caps_to_escape();
        profile.assignments[0].pass_through = vec![actions::PassThrough::Shift];
        let mut rig = rig(vec![profile]);
        rig.controller.input(InputEvent::KeyDown(106), ms(0), &rig.config);
        rig.output.take();
        rig.log.lock().unwrap().fail_writes = true;
        rig.controller.tick(ms(16), &[], &rig.config);
        assert!(matches!(rig.controller.state().device, DeviceState::Paused { .. }));
        assert_eq!(rig.output.take(), [OutputCommand::CancelAll]);
        let fresh = Arc::new(Mutex::new(Log::default()));
        rig.controller.attach(Box::new(FakeLink(fresh)), info(None), ms(32));
        rig.controller.input(InputEvent::KeyDown(57), ms(40), &rig.config);
        assert!(matches!(rig.output.take().as_slice(), [OutputCommand::Press { source: 57, .. }]));
    }

    #[test]
    fn lighting_triggers_show_a_disabled_layer_while_held() {
        let red = Rgb::new(255, 0, 0);
        let mut alert = engine::LayerConfig::new(
            "alert",
            engine::Zone::Named(engine::NamedZone::All),
            engine::EffectConfig::Solid { color: red },
        );
        alert.enabled = false;
        let trigger = Action::LightingTrigger { layer: "alert".into(), duration_ms: None };
        let profile = Profile { layers: vec![alert], ..assigned("default", "g1", trigger) };
        let mut rig = rig(vec![profile]);
        let frame_key = |rig: &Rig| rig.log.lock().unwrap().last_frame.unwrap()[4];
        rig.controller.tick(ms(10), &[], &rig.config);
        assert_eq!(frame_key(&rig), Rgb::BLACK);
        rig.controller.input(InputEvent::KeyDown(led::G1), ms(15), &rig.config);
        rig.controller.tick(ms(20), &[], &rig.config);
        assert_eq!(frame_key(&rig), red);
        rig.controller.input(InputEvent::KeyUp(led::G1), ms(25), &rig.config);
        rig.controller.tick(ms(30), &[], &rig.config);
        assert_eq!(frame_key(&rig), Rgb::BLACK);
    }

    #[test]
    fn reloading_an_unchanged_profile_leaves_held_keys_alone() {
        let mut rig = rig(vec![caps_to_escape()]);
        let writes = rig.log.lock().unwrap().mappings.len();
        rig.controller.input(InputEvent::KeyDown(57), ms(0), &rig.config);
        rig.output.take();
        rig.controller.reload(vec![caps_to_escape()], vec![], vec![]);
        rig.controller.input(InputEvent::KeyUp(57), ms(10), &rig.config);
        assert_eq!(rig.output.take(), [OutputCommand::Release { source: 57 }]);
        assert_eq!(rig.log.lock().unwrap().mappings.len(), writes);
    }

    #[test]
    fn reloading_a_changed_profile_cancels_held_keys_and_rewrites_mappings() {
        let mut rig = rig(vec![caps_to_escape()]);
        let writes = rig.log.lock().unwrap().mappings.len();
        rig.controller.input(InputEvent::KeyDown(57), ms(0), &rig.config);
        rig.output.take();
        let changed = assigned("default", "caps_lock", Action::Remap { key: key("space"), modifiers: vec![] });
        rig.controller.reload(vec![changed], vec![], vec![]);
        assert_eq!(rig.output.take(), [OutputCommand::CancelAll]);
        assert_eq!(rig.log.lock().unwrap().mappings.len(), writes + 1);
        rig.controller.take_events();
        tap_key(&mut rig, 57, 20, 30);
        let press = OutputCommand::Press { source: 57, program: actions::Program::remap(key("space"), &[]) };
        assert_eq!(rig.output.take(), [press, OutputCommand::Release { source: 57 }]);
    }

    #[test]
    fn a_content_only_change_to_the_active_profile_announces_an_edit() {
        let mut rig = rig(vec![caps_to_escape()]);
        rig.controller.take_events();
        rig.controller.reload(vec![caps_to_escape()], vec![], vec![]);
        assert_eq!(rig.controller.take_events(), []);
        let changed = assigned("default", "caps_lock", Action::Remap { key: key("space"), modifiers: vec![] });
        rig.controller.reload(vec![changed.clone()], vec![], vec![]);
        assert_eq!(rig.controller.take_events(), [Event::ProfileEdited { name: "default".into() }]);
        let saved = Profile { apps: vec!["game.exe".into()], ..changed };
        assert_eq!(rig.controller.handle(Request::SaveProfile { profile: saved }, &rig.config), Reply::Done);
        assert_eq!(rig.controller.take_events(), [Event::ProfileEdited { name: "default".into() }]);
    }

    #[test]
    fn editing_an_inactive_profile_announces_nothing() {
        let (mut controller, _log, _temp, config) = controller();
        controller.take_events();
        let other = Profile { apps: vec!["game.exe".into()], ..profile("games") };
        assert_eq!(controller.handle(Request::SaveProfile { profile: other }, &config), Reply::Done);
        assert_eq!(controller.take_events(), []);
    }

    #[test]
    fn a_dropped_link_clears_the_reported_winlock() {
        let mut rig = rig(vec![profile("default")]);
        assert_eq!(rig.controller.state().winlock, Some(false));
        rig.controller.set_device_state(DeviceState::Disconnected);
        assert_eq!(rig.controller.state().winlock, None);
    }

    #[test]
    fn shutdown_lets_go_of_held_keys() {
        let mut rig = rig(vec![caps_to_escape()]);
        rig.controller.input(InputEvent::KeyDown(57), ms(0), &rig.config);
        rig.output.take();
        rig.controller.shutdown();
        assert_eq!(rig.output.take(), [OutputCommand::CancelAll]);
        assert!(rig.log.lock().unwrap().released);
    }

    #[test]
    fn releasing_the_keyboard_forgets_key_state() {
        let mut profile = caps_to_escape();
        profile.assignments[0].pass_through = vec![actions::PassThrough::Shift];
        let mut rig = rig(vec![profile]);
        rig.controller.input(InputEvent::KeyDown(106), ms(0), &rig.config);
        rig.controller.handle(Request::Release, &rig.config);
        rig.output.take();
        let fresh = Arc::new(Mutex::new(Log::default()));
        rig.controller.attach(Box::new(FakeLink(fresh)), info(None), ms(32));
        rig.controller.input(InputEvent::KeyDown(57), ms(40), &rig.config);
        assert!(matches!(rig.output.take().as_slice(), [OutputCommand::Press { source: 57, .. }]));
    }

    fn linked_games(games: Profile) -> Rig {
        let games = Profile { apps: vec!["game.exe".into()], ..games };
        let mut rig = rig(vec![profile("default"), games]);
        rig.controller.focus(Some(r"C:\Games\game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "games");
        rig.output.take();
        rig
    }

    #[test]
    fn focus_switches_wait_while_output_is_busy() {
        let mut rig = linked_games(profile("games"));
        rig.output.set_busy(true);
        rig.controller.focus(Some(r"C:\Windows\notepad.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "games");
        assert!(rig.output.take().is_empty(), "nothing cancelled while busy");
        rig.controller.tick(ms(16), &[], &rig.config);
        assert_eq!(rig.controller.state().active_profile, "games");
        rig.output.set_busy(false);
        rig.controller.tick(ms(32), &[], &rig.config);
        assert_eq!(rig.controller.state().active_profile, "default");
        assert_eq!(rig.output.take(), [OutputCommand::CancelAll]);
    }

    #[test]
    fn focus_switches_wait_for_a_held_action_key() {
        let remap = Action::Remap { key: key("escape"), modifiers: vec![] };
        let mut rig = linked_games(assigned("games", "caps_lock", remap));
        rig.controller.input(InputEvent::KeyDown(57), ms(0), &rig.config);
        rig.controller.focus(Some(r"C:\Windows\notepad.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "games");
        rig.controller.input(InputEvent::KeyUp(57), ms(10), &rig.config);
        assert!(!rig.output.take().contains(&OutputCommand::CancelAll));
        rig.controller.tick(ms(16), &[], &rig.config);
        assert_eq!(rig.controller.state().active_profile, "default");
    }

    #[test]
    fn without_an_input_sink_only_output_free_keys_are_suppressed() {
        let remap = Action::Remap { key: key("escape"), modifiers: vec![] };
        let profile = Profile {
            assignments: vec![
                Assignment { key: key("caps_lock"), action: remap, pass_through: vec![] },
                Assignment { key: key("g1"), action: Action::NextProfile, pass_through: vec![] },
            ],
            ..profile("default")
        };
        let mut rig = rig(vec![profile]);
        rig.controller.set_output_available(false);
        let log = rig.log.lock().unwrap();
        let bytes = log.mappings.last().unwrap();
        assert_eq!((bytes[57], bytes[131]), (0x01, 0x00));
    }

    fn arm_recording(rig: &mut Rig) {
        tap_key(rig, led::DIAL_KEY, 0, 50);
        rig.controller.input(InputEvent::KeyDown(led::DIAL_KEY), ms(100), &rig.config);
        rig.controller.tick(ms(700), &[], &rig.config);
        rig.controller.input(InputEvent::KeyUp(led::DIAL_KEY), ms(750), &rig.config);
        assert_eq!(rig.controller.state().recording, RecorderState::AwaitingKey);
        rig.controller.take_events();
    }

    #[test]
    fn switching_profiles_cancels_a_recording() {
        let mut rig = rig(vec![profile("default"), profile("games")]);
        arm_recording(&mut rig);
        rig.controller.handle(Request::SetActiveProfile { name: "games".into() }, &rig.config);
        assert_eq!(rig.controller.state().recording, RecorderState::Idle);
        assert!(rig.controller.take_events().contains(&Event::Recording { state: RecorderState::Idle }));
    }

    #[test]
    fn dropping_the_link_cancels_a_recording() {
        let mut rig = rig(vec![profile("default")]);
        arm_recording(&mut rig);
        rig.controller.set_device_state(DeviceState::Disconnected);
        assert_eq!(rig.controller.state().recording, RecorderState::Idle);
        assert!(rig.controller.take_events().contains(&Event::Recording { state: RecorderState::Idle }));
    }

    #[test]
    fn brightness_is_not_saved_over_a_broken_settings_file() {
        let mut rig = rig(vec![profile("default")]);
        let path = rig.config.root().join("settings.toml");
        std::fs::write(&path, "brightness = [").unwrap();
        rig.controller.reload_settings(rig.config.load_settings());
        rig.controller.input(InputEvent::DialCw, ms(0), &rig.config);
        rig.controller.tick(ms(1100), &[], &rig.config);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "brightness = [");
        assert_eq!(rig.controller.state().brightness, 84);
        std::fs::write(&path, "fps = 30\n").unwrap();
        rig.controller.reload_settings(rig.config.load_settings());
        rig.controller.tick(ms(1200), &[], &rig.config);
        let saved = rig.config.load_settings().unwrap();
        assert_eq!((saved.brightness, saved.fps), (84, 30));
    }

    #[test]
    fn a_continuous_dial_ring_lights_from_the_start() {
        let settings = Settings { dial_ring: DialRing::Continuous, ..Settings::default() };
        let mut rig = rig_with(settings, vec![profile("default")]);
        rig.controller.tick(ms(16), &[], &rig.config);
        let brightness_blue = IndicatorRecord { led: led::DIAL_KEY, color: Rgb::new(0, 0xBF, 0xFF), alpha: 0xFF };
        assert!(rig.log.lock().unwrap().indicators.contains(&brightness_blue));
    }

    #[test]
    fn debounce_is_written_to_both_press_and_release_and_saved() {
        let mut rig = rig(vec![profile("default")]);
        rig.log.lock().unwrap().properties.clear();
        let reply = rig.controller.handle(Request::SetDebounce { enabled: true, timeout: 45 }, &rig.config);
        assert_eq!(reply, Reply::Done);
        assert_eq!(rig.log.lock().unwrap().properties, [(0x09, 1), (0x0A, 45), (0x39, 1), (0x38, 45)]);
        let saved = rig.config.load_settings().unwrap();
        assert_eq!(saved.debounce, Some(ipc::Debounce { enabled: true, timeout: 45 }));
        let too_long = rig.controller.handle(Request::SetDebounce { enabled: true, timeout: 301 }, &rig.config);
        assert!(matches!(too_long, Reply::Error { message } if message.contains("0 to 300")));
    }

    #[test]
    fn a_saved_debounce_is_applied_when_the_keyboard_attaches() {
        let settings =
            Settings { debounce: Some(ipc::Debounce { enabled: false, timeout: 30 }), ..Settings::default() };
        let rig = rig_with(settings, vec![profile("default")]);
        assert!(rig.log.lock().unwrap().properties.contains(&(0x0A, 30)));
    }

    #[test]
    fn get_settings_returns_the_current_settings() {
        let mut rig = rig(vec![profile("default")]);
        let Reply::Settings { settings } = rig.controller.handle(Request::GetSettings, &rig.config) else {
            panic!("expected settings");
        };
        assert_eq!(settings.brightness, 80);
    }

    #[test]
    fn set_settings_applies_only_the_named_fields_and_persists_them() {
        let mut rig = rig(vec![profile("default"), profile("games")]);
        let patch = SettingsPatch {
            winlock_disables: Some(vec![ipc::WinLockShortcut::Win, ipc::WinLockShortcut::AltF4]),
            dial_ring: Some(DialRing::Continuous),
            ..SettingsPatch::default()
        };
        assert_eq!(rig.controller.handle(Request::SetSettings { patch }, &rig.config), Reply::Done);
        assert!(rig.log.lock().unwrap().properties.contains(&(0x4A, 5)));
        let saved = rig.config.load_settings().unwrap();
        assert_eq!(saved.dial_ring, DialRing::Continuous);
        assert_eq!(saved.polling_rate_hz, None);
        let unknown = SettingsPatch { default_profile: Some("nope".into()), ..SettingsPatch::default() };
        let reply = rig.controller.handle(Request::SetSettings { patch: unknown }, &rig.config);
        assert!(matches!(reply, Reply::Error { message } if message.contains("nope")));
    }

    #[test]
    fn turning_auto_switch_back_on_applies_the_focused_apps_profile() {
        let games = Profile { apps: vec!["game.exe".into()], ..profile("games") };
        let settings = Settings { auto_switch: false, ..Settings::default() };
        let mut rig = rig_with(settings, vec![profile("default"), games]);
        rig.controller.focus(Some(r"C:\Games\game.exe".into()));
        assert_eq!(rig.controller.state().active_profile, "default");
        let patch = SettingsPatch { auto_switch: Some(true), ..SettingsPatch::default() };
        rig.controller.handle(Request::SetSettings { patch }, &rig.config);
        assert_eq!(rig.controller.state().active_profile, "games");
    }

    #[test]
    fn changing_the_default_profile_takes_effect_without_a_manual_choice() {
        let mut rig = rig(vec![profile("default"), profile("games")]);
        let patch = SettingsPatch { default_profile: Some("games".into()), ..SettingsPatch::default() };
        rig.controller.handle(Request::SetSettings { patch }, &rig.config);
        assert_eq!(rig.controller.state().active_profile, "games");
    }

    #[test]
    fn the_gui_can_start_and_cancel_a_recording() {
        let mut rig = rig(vec![profile("default")]);
        assert_eq!(rig.controller.handle(Request::StartRecording { key: None }, &rig.config), Reply::Done);
        assert_eq!(rig.controller.state().recording, RecorderState::AwaitingKey);
        assert!(rig.controller.take_events().contains(&Event::Recording { state: RecorderState::AwaitingKey }));
        assert_eq!(rig.controller.handle(Request::CancelRecording, &rig.config), Reply::Done);
        assert_eq!(rig.controller.state().recording, RecorderState::Idle);
        tap_key(&mut rig, led::G1, 0, 10);
        assert_eq!(rig.controller.state().recording, RecorderState::Idle);
    }

    const G2: u8 = led::G1 + 1;

    #[test]
    fn a_keyed_recording_starts_at_once_and_stop_saves_it_like_the_target_key() {
        let mut rig = rig(vec![profile("default")]);
        let start = Request::StartRecording { key: Some(G2) };
        assert_eq!(rig.controller.handle(start, &rig.config), Reply::Done);
        let recording = RecorderState::Recording { key: G2 };
        assert_eq!(rig.controller.state().recording, recording);
        assert!(rig.controller.take_events().contains(&Event::Recording { state: recording }));
        tap_key(&mut rig, 4, 100, 160);
        assert_eq!(rig.controller.handle(Request::StopRecording, &rig.config), Reply::Done);
        let steps = vec![MacroStep::Down(key("a")), MacroStep::Delay(60), MacroStep::Up(key("a"))];
        let expected = Assignment {
            key: key("g2"),
            action: Action::Macro { steps, mode: RepeatMode::Once },
            pass_through: vec![],
        };
        assert_eq!(rig.config.load_profiles().0[0].assignments, [expected]);
        let events = rig.controller.take_events();
        let macro_at = events.iter().position(|event| *event == Event::MacroRecorded { key: "g2".into() });
        let idle_at = events.iter().position(|event| *event == Event::Recording { state: RecorderState::Idle });
        assert!(macro_at.unwrap() < idle_at.unwrap(), "{events:?}");
        assert_eq!(rig.controller.state().recording, RecorderState::Idle);
    }

    #[test]
    fn stopping_when_nothing_is_recorded_is_a_quiet_no_op() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.take_events();
        assert_eq!(rig.controller.handle(Request::StopRecording, &rig.config), Reply::Done);
        assert!(rig.controller.take_events().is_empty());
        assert!(rig.config.load_profiles().0.is_empty(), "nothing was written");
    }

    #[test]
    fn a_keyed_recording_refuses_keys_that_cannot_hold_a_macro() {
        let mut rig = rig(vec![profile("default")]);
        let reply = rig.controller.handle(Request::StartRecording { key: Some(led::DIAL_KEY) }, &rig.config);
        assert!(matches!(reply, Reply::Error { message } if message.contains("cannot be reassigned")));
        assert_eq!(rig.controller.state().recording, RecorderState::Idle);
    }

    #[derive(Clone, Default)]
    struct FakeAutostart(Arc<AtomicBool>);

    impl install::Autostart for FakeAutostart {
        fn enable(&self) -> io::Result<()> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn disable(&self) -> io::Result<()> {
            self.0.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn is_enabled(&self) -> io::Result<bool> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }

    #[test]
    fn autostart_is_toggled_and_reported() {
        let mut rig = rig(vec![profile("default")]);
        let fake = FakeAutostart::default();
        rig.controller.set_autostart(Box::new(fake.clone()));
        assert!(!rig.controller.state().autostart);
        assert_eq!(rig.controller.handle(Request::SetAutostart { enabled: true }, &rig.config), Reply::Done);
        assert!(rig.controller.state().autostart);
        assert_eq!(rig.controller.handle(Request::SetAutostart { enabled: false }, &rig.config), Reply::Done);
        assert!(!fake.0.load(Ordering::SeqCst));
    }

    #[test]
    fn autostart_failures_are_reported_to_the_client() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.set_autostart(Box::new(install::Unavailable("k100 not found".into())));
        let reply = rig.controller.handle(Request::SetAutostart { enabled: true }, &rig.config);
        assert!(matches!(reply, Reply::Error { message } if message.contains("k100 not found")));
    }

    #[test]
    fn recording_needs_a_connected_keyboard() {
        let mut controller =
            Controller::new(Settings::default(), vec![profile("default")], vec![], Box::new(DiscardOutput));
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(dir.path().to_path_buf());
        let reply = controller.handle(Request::StartRecording { key: None }, &config);
        assert!(matches!(reply, Reply::Error { message } if message.contains("no keyboard")));
    }

    #[test]
    fn state_reports_the_keyboards_maximum_polling_rate() {
        let rig = rig(vec![profile("default")]);
        assert_eq!(rig.controller.state().max_polling_rate_hz, Some(8000));
    }

    fn break_settings_file(rig: &Rig) -> (std::path::PathBuf, &'static str) {
        let path = rig.config.root().join("settings.toml");
        let broken = "brightness = [";
        std::fs::write(&path, broken).unwrap();
        (path, broken)
    }

    #[test]
    fn settings_writes_refuse_to_replace_an_unparseable_file() {
        let mut rig = rig(vec![profile("default")]);
        let (path, broken) = break_settings_file(&rig);
        rig.controller.reload_settings(rig.config.load_settings());
        rig.log.lock().unwrap().properties.clear();
        let patch = SettingsPatch { auto_switch: Some(false), ..SettingsPatch::default() };
        let requests = [
            Request::SetSettings { patch },
            Request::SetDebounce { enabled: true, timeout: 30 },
            Request::SetPollingRate { hz: 500 },
            Request::SetBrightness { percent: 10 },
        ];
        for request in requests {
            let reply = rig.controller.handle(request.clone(), &rig.config);
            assert!(
                matches!(reply, Reply::Error { ref message } if message.contains("settings.toml")),
                "{request:?}: {reply:?}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), broken, "{request:?}");
        }
        assert!(rig.log.lock().unwrap().properties.is_empty());
        assert!(rig.controller.state().config_errors.iter().any(|e| e.contains("settings.toml")));
    }

    #[test]
    fn an_out_of_range_debounce_in_the_settings_file_never_reaches_the_keyboard() {
        let mut rig = rig(vec![profile("default")]);
        rig.log.lock().unwrap().properties.clear();
        let settings =
            Settings { debounce: Some(ipc::Debounce { enabled: true, timeout: 4_000_000_000 }), ..Settings::default() };
        rig.controller.reload_settings(Ok(settings));
        let properties = rig.log.lock().unwrap().properties.clone();
        assert!(properties.iter().all(|(id, _)| ![0x09, 0x0A, 0x38, 0x39].contains(id)), "{properties:?}");
        assert!(rig.controller.state().config_errors.iter().any(|e| e.contains("0 to 300")));
    }

    #[test]
    fn an_out_of_range_saved_debounce_is_not_applied_on_attach() {
        let settings =
            Settings { debounce: Some(ipc::Debounce { enabled: true, timeout: 301 }), ..Settings::default() };
        let rig = rig_with(settings, vec![profile("default")]);
        assert!(rig.log.lock().unwrap().properties.iter().all(|(id, _)| ![0x09, 0x0A, 0x38, 0x39].contains(id)));
        assert!(rig.controller.state().config_errors.iter().any(|e| e.contains("0 to 300")));
    }

    #[test]
    fn a_debounce_save_keeps_direct_edits_pending_for_the_watcher() {
        let mut rig = rig(vec![profile("default")]);
        let edited = Settings { polling_rate_hz: Some(500), ..Settings::default() };
        rig.config.save_settings(&edited).unwrap();
        assert_eq!(
            rig.controller.handle(Request::SetDebounce { enabled: true, timeout: 30 }, &rig.config),
            Reply::Done
        );
        rig.log.lock().unwrap().properties.clear();
        rig.controller.reload_settings(rig.config.load_settings());
        assert!(rig.log.lock().unwrap().properties.contains(&(0x01, 3)));
    }

    #[test]
    fn a_debounce_save_keeps_a_pending_dial_brightness() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.input(InputEvent::DialCw, ms(0), &rig.config);
        assert_eq!(rig.controller.state().brightness, 84);
        assert_eq!(
            rig.controller.handle(Request::SetDebounce { enabled: true, timeout: 30 }, &rig.config),
            Reply::Done
        );
        assert_eq!(rig.controller.state().brightness, 84);
    }

    #[test]
    fn a_debounce_result_does_not_disturb_a_polling_warning() {
        let mut rig = rig(vec![profile("default")]);
        let bad_polling = Settings { polling_rate_hz: Some(300), ..Settings::default() };
        rig.controller.reload_settings(Ok(bad_polling.clone()));
        let polling_warning = rig.controller.state().config_errors;
        assert_eq!(polling_warning.len(), 1);
        let with_debounce = Settings { debounce: Some(ipc::Debounce { enabled: true, timeout: 30 }), ..bad_polling };
        rig.controller.reload_settings(Ok(with_debounce));
        assert_eq!(rig.controller.state().config_errors, polling_warning);
    }

    #[test]
    fn a_brightness_save_keeps_a_direct_polling_edit_pending() {
        let mut rig = rig(vec![profile("default")]);
        let edited = Settings { polling_rate_hz: Some(500), ..Settings::default() };
        rig.config.save_settings(&edited).unwrap();
        assert_eq!(rig.controller.handle(Request::SetBrightness { percent: 40 }, &rig.config), Reply::Done);
        rig.log.lock().unwrap().properties.clear();
        rig.controller.reload_settings(rig.config.load_settings());
        assert!(rig.log.lock().unwrap().properties.contains(&(0x01, 3)));
    }

    #[test]
    fn a_polling_save_keeps_a_direct_dial_ring_edit_pending() {
        let mut rig = rig(vec![profile("default")]);
        let edited = Settings { dial_ring: DialRing::Continuous, ..Settings::default() };
        rig.config.save_settings(&edited).unwrap();
        assert_eq!(rig.controller.handle(Request::SetPollingRate { hz: 1000 }, &rig.config), Reply::Done);
        let ring = |rig: &mut Rig| match rig.controller.handle(Request::GetSettings, &rig.config) {
            Reply::Settings { settings } => settings.dial_ring,
            other => panic!("expected settings, got {other:?}"),
        };
        assert_eq!(ring(&mut rig), DialRing::Temporary);
        rig.controller.reload_settings(rig.config.load_settings());
        assert_eq!(ring(&mut rig), DialRing::Continuous);
    }

    #[test]
    fn an_explicit_brightness_supersedes_a_pending_dial_change() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.input(InputEvent::DialCw, ms(0), &rig.config);
        assert_eq!(rig.controller.handle(Request::SetBrightness { percent: 40 }, &rig.config), Reply::Done);
        let edited = Settings { brightness: 70, ..Settings::default() };
        rig.config.save_settings(&edited).unwrap();
        rig.controller.reload_settings(rig.config.load_settings());
        assert_eq!(rig.controller.state().brightness, 70);
    }

    #[test]
    fn a_settled_dial_brightness_save_keeps_a_direct_polling_edit_pending() {
        let mut rig = rig(vec![profile("default")]);
        rig.controller.input(InputEvent::DialCw, ms(0), &rig.config);
        let edited = Settings { polling_rate_hz: Some(500), ..Settings::default() };
        rig.config.save_settings(&edited).unwrap();
        rig.controller.tick(ms(1100), &[], &rig.config);
        assert_eq!(rig.config.load_settings().unwrap().brightness, 84);
        rig.log.lock().unwrap().properties.clear();
        rig.controller.reload_settings(rig.config.load_settings());
        assert!(rig.log.lock().unwrap().properties.contains(&(0x01, 3)));
    }
}
