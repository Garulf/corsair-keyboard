use std::collections::BTreeMap;
use std::time::Duration;

use bragi::{PollingRate, PropertyId};
use engine::{Compositor, KeyPress, Layout};
use ipc::{DaemonState, DeviceState, Event, PROTOCOL_VERSION, Profile, Reply, Request};
use k100::{DeviceError, Frame, IndicatorRecord, KeyMappings, Session, Transport};

use crate::config::{ConfigDir, Settings, default_profile, profile_file_name};

const BRIGHTNESS_SCALE: u32 = 10;

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
    chosen: Option<String>,
    compositor: Compositor,
    link: Option<Box<dyn DeviceLink>>,
    max_polling_rate: Option<PollingRate>,
    device: DeviceState,
    last_ping: Duration,
    profile_errors: Vec<String>,
    settings_error: Option<String>,
    device_warning: Option<String>,
    events: Vec<Event>,
}

impl Controller {
    pub fn new(settings: Settings, profiles: Vec<Profile>, config_errors: Vec<String>) -> Self {
        let mut profiles: BTreeMap<String, Profile> = profiles.into_iter().map(|p| (p.name.clone(), p)).collect();
        if profiles.is_empty() {
            let fallback = default_profile();
            profiles.insert(fallback.name.clone(), fallback);
        }
        let active = pick_active(&profiles, None, &settings.default_profile);
        let compositor = Compositor::new(Layout::ansi(), &profiles[&active].layers);
        Self {
            settings,
            profiles,
            active,
            chosen: None,
            compositor,
            link: None,
            max_polling_rate: None,
            device: DeviceState::Disconnected,
            last_ping: Duration::ZERO,
            profile_errors: config_errors,
            settings_error: None,
            device_warning: None,
            events: Vec::new(),
        }
    }

    pub fn state(&self) -> DaemonState {
        let mut config_errors: Vec<String> = self.settings_error.iter().cloned().collect();
        config_errors.extend(self.profile_errors.iter().cloned());
        config_errors.extend(self.device_warning.iter().cloned());
        DaemonState {
            device: self.device.clone(),
            active_profile: self.active.clone(),
            profiles: self.profiles.keys().cloned().collect(),
            brightness: self.settings.brightness.min(100),
            polling_rate_hz: self.settings.polling_rate_hz,
            config_errors,
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
        if self.link.is_some() {
            self.set_state(DeviceState::Connected { model: info.model, firmware: info.firmware });
        }
    }

    pub fn set_device_state(&mut self, state: DeviceState) {
        self.link = None;
        self.set_state(state);
    }

    pub fn tick(&mut self, now: Duration, presses: &[KeyPress]) {
        if self.link.is_none() {
            return;
        }
        let frame = self.compositor.render(now, presses);
        let _ = self.with_link(|link| link.write_frame(&frame));
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
        let active = pick_active(&self.profiles, self.chosen.as_deref(), &self.settings.default_profile);
        self.activate(active);
    }

    pub fn reload_settings(&mut self, settings: Result<Settings, String>) {
        match settings {
            Ok(settings) => {
                self.settings_error = None;
                let polling_changed = settings.polling_rate_hz != self.settings.polling_rate_hz;
                let brightness_changed = settings.brightness != self.settings.brightness;
                self.settings = settings;
                if brightness_changed {
                    let _ = self.apply_brightness();
                }
                if let (true, Some(hz)) = (polling_changed, self.settings.polling_rate_hz) {
                    self.device_warning = self.apply_polling_rate(hz).err();
                }
            }
            Err(message) => self.settings_error = Some(message),
        }
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn shutdown(&mut self) {
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
        self.compositor = Compositor::new(Layout::ansi(), &self.active_profile().layers);
        if changed {
            self.events.push(Event::ProfileChanged { name: self.active.clone() });
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
                self.link = None;
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

    fn controller() -> (Controller, Arc<Mutex<Log>>, tempfile::TempDir, ConfigDir) {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let settings = Settings { brightness: 80, ..Settings::default() };
        let mut controller = Controller::new(settings, vec![profile("default"), profile("games")], vec![]);
        let log = Arc::new(Mutex::new(Log::default()));
        controller.attach(Box::new(FakeLink(log.clone())), info(Some(PollingRate::Hz8000)), Duration::ZERO);
        (controller, log, temp, config)
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
        let (mut controller, log, _temp, _config) = controller();
        controller.tick(Duration::from_millis(16), &[]);
        controller.tick(Duration::from_secs(10), &[]);
        assert_eq!(log.lock().unwrap().pings, 0);
        controller.tick(Duration::from_secs(25), &[]);
        let log = log.lock().unwrap();
        assert_eq!(log.frames, 3);
        assert_eq!(log.pings, 1);
    }

    #[test]
    fn write_failure_pauses_and_drops_the_link() {
        let (mut controller, log, _temp, _config) = controller();
        log.lock().unwrap().fail_writes = true;
        controller.tick(Duration::from_millis(16), &[]);
        assert!(matches!(controller.state().device, DeviceState::Paused { .. }));
        assert!(controller.wants_device());
        log.lock().unwrap().fail_writes = false;
        controller.tick(Duration::from_millis(32), &[]);
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
        let controller = Controller::new(Settings::default(), vec![], vec![]);
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
        controller.tick(Duration::from_millis(16), &[]);
        assert_eq!(log.lock().unwrap().frames, 1);
    }

    #[test]
    fn polling_above_the_keyboard_maximum_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let mut controller = Controller::new(Settings::default(), vec![profile("default")], vec![]);
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
}
