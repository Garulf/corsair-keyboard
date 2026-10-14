use std::collections::BTreeMap;
use std::time::Duration;

use bragi::{PollingRate, PropertyId};
use engine::{Compositor, KeyPress, Layout};
use ipc::{DaemonState, DeviceState, Event, PROTOCOL_VERSION, Profile, Reply, Request};
use k100::{DeviceError, Frame, Session, Transport};

use crate::config::{ConfigDir, Settings, default_profile};

const BRIGHTNESS_SCALE: u32 = 10;

pub trait DeviceLink: Send {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), DeviceError>;
    fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError>;
    fn ping(&mut self) -> Result<(), DeviceError>;
    fn release(&mut self) -> Result<(), DeviceError>;
}

impl<T: Transport> DeviceLink for Session<T> {
    fn write_frame(&mut self, frame: &Frame) -> Result<(), DeviceError> {
        Session::write_frame(self, frame)
    }

    fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
        Session::set_property(self, property, value)
    }

    fn ping(&mut self) -> Result<(), DeviceError> {
        Session::ping(self)
    }

    fn release(&mut self) -> Result<(), DeviceError> {
        Session::release(self)
    }
}

pub struct Controller {
    settings: Settings,
    profiles: BTreeMap<String, Profile>,
    active: String,
    compositor: Compositor,
    link: Option<Box<dyn DeviceLink>>,
    device: DeviceState,
    last_ping: Duration,
    config_errors: Vec<String>,
    events: Vec<Event>,
}

impl Controller {
    pub fn new(settings: Settings, profiles: Vec<Profile>, config_errors: Vec<String>) -> Self {
        let mut profiles: BTreeMap<String, Profile> = profiles.into_iter().map(|p| (p.name.clone(), p)).collect();
        if profiles.is_empty() {
            let fallback = default_profile();
            profiles.insert(fallback.name.clone(), fallback);
        }
        let active = pick_active(&profiles, &settings.default_profile, None);
        let compositor = Compositor::new(Layout::ansi(), &profiles[&active].layers);
        Self {
            settings,
            profiles,
            active,
            compositor,
            link: None,
            device: DeviceState::Disconnected,
            last_ping: Duration::ZERO,
            config_errors,
            events: Vec::new(),
        }
    }

    pub fn state(&self) -> DaemonState {
        DaemonState {
            device: self.device.clone(),
            active_profile: self.active.clone(),
            profiles: self.profiles.keys().cloned().collect(),
            brightness: self.active_profile().brightness,
            polling_rate_hz: self.settings.polling_rate_hz,
            config_errors: self.config_errors.clone(),
        }
    }

    pub fn wants_device(&self) -> bool {
        self.link.is_none() && self.device != DeviceState::Released
    }

    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs(1) / self.settings.fps.clamp(1, 200)
    }

    pub fn attach(&mut self, link: Box<dyn DeviceLink>, model: &str, firmware: String, now: Duration) {
        self.link = Some(link);
        self.last_ping = now;
        self.apply_brightness();
        if let Some(rate) = self.settings.polling_rate_hz.and_then(PollingRate::from_hz) {
            self.with_link(|link| link.set_property(PropertyId::POLLING_RATE, rate.raw()));
        }
        if self.link.is_some() {
            self.set_state(DeviceState::Connected { model: model.to_owned(), firmware });
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
        self.with_link(|link| link.write_frame(&frame));
        if now.saturating_sub(self.last_ping) >= k100::PING_INTERVAL {
            self.last_ping = now;
            self.with_link(|link| link.ping());
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
                self.activate(name);
                Reply::Done
            }
            Request::SetBrightness { percent } => {
                let mut profile = self.active_profile().clone();
                profile.brightness = percent.min(100);
                self.save_profile(profile, config)
            }
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

    pub fn reload(&mut self, profiles: Vec<Profile>, config_errors: Vec<String>) {
        self.config_errors = config_errors;
        if profiles.is_empty() {
            return;
        }
        self.profiles = profiles.into_iter().map(|p| (p.name.clone(), p)).collect();
        let active = pick_active(&self.profiles, &self.settings.default_profile, Some(&self.active));
        self.activate(active);
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
        self.apply_brightness();
        if changed {
            self.events.push(Event::ProfileChanged { name: self.active.clone() });
        }
    }

    fn save_profile(&mut self, profile: Profile, config: &ConfigDir) -> Reply {
        if profile.name.trim().is_empty() {
            return error("profile name must not be empty".into());
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
        if self.profiles.remove(name).is_none() {
            return error(format!("no profile named {name:?}"));
        }
        match config.delete_profile(name) {
            Ok(()) => Reply::Done,
            Err(failure) => error(format!("deleting profile: {failure}")),
        }
    }

    fn set_polling_rate(&mut self, hz: u32, config: &ConfigDir) -> Reply {
        let Some(rate) = PollingRate::from_hz(hz) else {
            return error(format!("unsupported polling rate {hz} Hz (125, 250, 500, 1000, 2000, 4000, 8000)"));
        };
        self.settings.polling_rate_hz = Some(hz);
        if let Err(failure) = config.save_settings(&self.settings) {
            return error(format!("saving settings: {failure}"));
        }
        self.with_link(|link| link.set_property(PropertyId::POLLING_RATE, rate.raw()));
        Reply::Done
    }

    fn release(&mut self) -> Reply {
        let result = self.link.take().map_or(Ok(()), |mut link| link.release());
        self.set_state(DeviceState::Released);
        match result {
            Ok(()) => Reply::Done,
            Err(failure) => error(format!("release reported: {failure}")),
        }
    }

    fn apply_brightness(&mut self) {
        let value = u32::from(self.active_profile().brightness.min(100)) * BRIGHTNESS_SCALE;
        self.with_link(|link| link.set_property(PropertyId::BRIGHTNESS, value));
    }

    fn with_link(&mut self, operation: impl FnOnce(&mut dyn DeviceLink) -> Result<(), DeviceError>) {
        let Some(link) = self.link.as_deref_mut() else { return };
        if let Err(failure) = operation(link) {
            self.link = None;
            self.set_state(DeviceState::Paused { reason: failure.to_string() });
        }
    }

    fn set_state(&mut self, state: DeviceState) {
        if self.device != state {
            self.device = state.clone();
            self.events.push(Event::DeviceState { state });
        }
    }
}

fn pick_active(profiles: &BTreeMap<String, Profile>, preferred: &str, current: Option<&String>) -> String {
    current
        .filter(|name| profiles.contains_key(*name))
        .cloned()
        .or_else(|| profiles.contains_key(preferred).then(|| preferred.to_owned()))
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
        properties: Vec<(u16, u32)>,
        pings: usize,
        released: bool,
        fail_writes: bool,
    }

    struct FakeLink(Arc<Mutex<Log>>);

    impl DeviceLink for FakeLink {
        fn write_frame(&mut self, _frame: &Frame) -> Result<(), DeviceError> {
            let mut log = self.0.lock().unwrap();
            if log.fail_writes {
                return Err(DeviceError::Timeout(bragi::Command::WriteBufferBegin));
            }
            log.frames += 1;
            Ok(())
        }
        fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
            self.0.lock().unwrap().properties.push((property.0, value));
            Ok(())
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

    fn profile(name: &str, brightness: u8) -> Profile {
        Profile { name: name.into(), brightness, ..default_profile() }
    }

    fn controller() -> (Controller, Arc<Mutex<Log>>, tempfile::TempDir, ConfigDir) {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        let mut controller =
            Controller::new(Settings::default(), vec![profile("default", 80), profile("games", 30)], vec![]);
        let log = Arc::new(Mutex::new(Log::default()));
        controller.attach(Box::new(FakeLink(log.clone())), "K100", "1.15.61".into(), Duration::ZERO);
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
    fn switching_profiles_applies_brightness_and_announces() {
        let (mut controller, log, _temp, config) = controller();
        controller.take_events();
        let reply = controller.handle(Request::SetActiveProfile { name: "games".into() }, &config);
        assert_eq!(reply, Reply::Done);
        assert_eq!(controller.state().active_profile, "games");
        assert_eq!(log.lock().unwrap().properties.last(), Some(&(0x02, 300)));
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
        assert_eq!(config.load_profiles().0[0].brightness, 100);
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
    fn reload_keeps_a_valid_active_profile_or_falls_back() {
        let (mut controller, _log, _temp, config) = controller();
        controller.handle(Request::SetActiveProfile { name: "games".into() }, &config);
        controller.reload(vec![profile("default", 50)], vec!["games.toml: broken".into()]);
        assert_eq!(controller.state().active_profile, "default");
        assert_eq!(controller.state().config_errors, ["games.toml: broken"]);
    }

    #[test]
    fn new_without_profiles_uses_the_built_in_default() {
        let controller = Controller::new(Settings::default(), vec![], vec![]);
        assert_eq!(controller.state().active_profile, "default");
        assert_eq!(controller.state().device, DeviceState::Disconnected);
    }
}
