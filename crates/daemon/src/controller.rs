use std::collections::BTreeMap;
use std::time::Duration;

use actions::{
    Action, Assignment, BrightnessChange, Dispatcher, Intent, Key, MacroStep, OutputCommand, ProfileTarget, RepeatMode,
};
use bragi::{PollingRate, PropertyId};
use engine::{Compositor, KeyPress, Layout};
use ipc::{DaemonState, DeviceState, Event, PROTOCOL_VERSION, Profile, Reply, Request};
use k100::{DeviceError, Frame, IndicatorRecord, InputEvent, KeyMappings, Session, Transport, led};

use crate::config::{ConfigDir, Settings, default_profile, profile_file_name};
use crate::indicators::Indicators;

const BRIGHTNESS_SCALE: u32 = 10;
const BRIGHTNESS_STEP: u8 = 4;
const BRIGHTNESS_SAVE_DELAY: Duration = Duration::from_secs(1);
const WINLOCK_SETTLE: Duration = Duration::from_millis(150);

pub trait OutputPort: Send {
    fn send(&mut self, command: OutputCommand);
}

impl OutputPort for actions::OutputHandle {
    fn send(&mut self, command: OutputCommand) {
        actions::OutputHandle::send(self, command);
    }
}

pub struct DiscardOutput;

impl OutputPort for DiscardOutput {
    fn send(&mut self, _command: OutputCommand) {}
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
    compositor: Compositor,
    dispatcher: Dispatcher,
    output: Box<dyn OutputPort>,
    indicators: Indicators,
    link: Option<Box<dyn DeviceLink>>,
    max_polling_rate: Option<PollingRate>,
    device: DeviceState,
    last_ping: Duration,
    profile_errors: Vec<String>,
    settings_error: Option<String>,
    device_warning: Option<String>,
    warnings: Vec<String>,
    winlock: Option<bool>,
    winlock_check: Option<Duration>,
    brightness_changed_at: Option<Duration>,
    now: Duration,
    events: Vec<Event>,
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
            compositor,
            dispatcher,
            output,
            indicators,
            link: None,
            max_polling_rate: None,
            device: DeviceState::Disconnected,
            last_ping: Duration::ZERO,
            profile_errors: config_errors,
            settings_error: None,
            device_warning: None,
            warnings: Vec::new(),
            winlock: None,
            winlock_check: None,
            brightness_changed_at: None,
            now: Duration::ZERO,
            events: Vec::new(),
        }
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    pub fn state(&self) -> DaemonState {
        let mut config_errors: Vec<String> = self.settings_error.iter().cloned().collect();
        config_errors.extend(self.profile_errors.iter().cloned());
        config_errors.extend(self.device_warning.iter().cloned());
        config_errors.extend(self.warnings.iter().cloned());
        DaemonState {
            device: self.device.clone(),
            active_profile: self.active.clone(),
            profiles: self.profiles.keys().cloned().collect(),
            brightness: self.settings.brightness.min(100),
            polling_rate_hz: self.settings.polling_rate_hz,
            config_errors,
            winlock: self.winlock,
            dial_mode: self.dispatcher.dial_mode().label(),
            recording: self.dispatcher.recorder_state(),
        }
    }

    pub fn wants_device(&self) -> bool {
        self.link.is_none() && self.device != DeviceState::Released
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
        if let Some(hz) = self.settings.polling_rate_hz {
            if let Err(message) = self.apply_polling_rate(hz) {
                self.device_warning = Some(message);
            }
        }
        self.indicators.reset();
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
        if self.winlock_check.is_some_and(|due| now >= due) {
            self.winlock_check = None;
            self.resolve_winlock();
        }
        self.save_brightness_when_settled(config);
        if self.link.is_none() {
            return;
        }
        let frame = self.compositor.render(now, presses);
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
                self.activate(name);
                Reply::Done
            }
            Request::SetBrightness { percent } => self.set_brightness(percent, config),
            Request::SetPollingRate { hz } => self.set_polling_rate(hz, config),
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
            Request::Subscribe => Reply::Done,
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
            }
            Err(message) => self.settings_error = Some(message),
        }
    }

    pub fn focus(&mut self, exe: Option<String>) {
        self.foreground = exe;
        if self.settings.auto_switch {
            let target = self.preferred_profile();
            self.activate(target);
        }
    }

    fn preferred_profile(&self) -> String {
        let focused = self
            .foreground
            .as_deref()
            .filter(|_| self.settings.auto_switch)
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
            self.indicators.profile_switched(self.now);
            self.events.push(Event::ProfileChanged { name: self.active.clone() });
        }
        self.write_key_mappings();
    }

    fn drop_link(&mut self) {
        if self.link.take().is_some() {
            self.output.send(OutputCommand::CancelAll);
            self.dispatcher.forget_held_keys();
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
        if !settled {
            return;
        }
        self.brightness_changed_at = None;
        let mut on_disk = config.load_settings().unwrap_or_else(|_| self.settings.clone());
        on_disk.brightness = self.settings.brightness;
        if config.save_settings(&on_disk).is_ok() {
            self.settings = on_disk;
        }
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
        let assignments = self.active_profile().assignments.clone();
        let result = self.with_link(|link| {
            let mappings: KeyMappings = actions::key_mappings(&assignments, link.key_count());
            link.set_key_mappings(&mappings)
        });
        if let Err(failure) = result {
            if failure.status().is_some() {
                self.device_warning = Some(format!("keyboard rejected the key assignments: {failure}"));
            }
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
        if let Err(message) = self.apply_polling_rate(hz) {
            return error(message);
        }
        let mut on_disk = config.load_settings().unwrap_or_else(|_| self.settings.clone());
        on_disk.polling_rate_hz = Some(hz);
        if let Err(failure) = config.save_settings(&on_disk) {
            return error(format!("saving settings: {failure}"));
        }
        self.settings = on_disk;
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

    fn release(&mut self) -> Reply {
        self.output.send(OutputCommand::CancelAll);
        self.dispatcher.forget_held_keys();
        self.winlock = None;
        self.winlock_check = None;
        let result = self.link.take().map_or(Ok(()), |mut link| link.release());
        self.set_state(DeviceState::Released);
        match result {
            Ok(()) => Reply::Done,
            Err(failure) => error(format!("release reported: {failure}")),
        }
    }

    fn set_brightness(&mut self, percent: u8, config: &ConfigDir) -> Reply {
        let mut on_disk = config.load_settings().unwrap_or_else(|_| self.settings.clone());
        on_disk.brightness = percent.min(100);
        if let Err(failure) = config.save_settings(&on_disk) {
            return error(format!("saving settings: {failure}"));
        }
        self.settings = on_disk;
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
        if let Err(failure) = &result {
            if failure.status().is_none() {
                self.drop_link();
                self.set_state(DeviceState::Paused { reason: failure.to_string() });
            }
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

#[cfg(test)]
mod tests {
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
    struct FakeOutput(Arc<Mutex<Vec<OutputCommand>>>);

    impl FakeOutput {
        fn take(&self) -> Vec<OutputCommand> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl OutputPort for FakeOutput {
        fn send(&mut self, command: OutputCommand) {
            self.0.lock().unwrap().push(command);
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
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let settings = Settings { brightness: 80, ..Settings::default() };
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
}
