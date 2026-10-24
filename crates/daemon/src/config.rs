use std::io;
use std::path::{Path, PathBuf};

use engine::{EffectConfig, LayerConfig, NamedZone, Zone};
pub use ipc::{Debounce, DialRing, Profile, Settings, WinLockShortcut};
use serde::Serialize;

const SETTINGS_FILE: &str = "settings.toml";
const PROFILES_DIR: &str = "profiles";
const LOGS_DIR: &str = "logs";
const SDK_TRACE_FILE: &str = "sdk-trace.jsonl";
const STANDIN_DIR: &str = "icue-standin";

fn default_profile_name() -> String {
    "default".into()
}

pub fn default_profile() -> Profile {
    Profile {
        version: 1,
        name: default_profile_name(),
        apps: Vec::new(),
        layers: vec![LayerConfig::new(
            "rainbow",
            Zone::Named(NamedZone::All),
            EffectConfig::RainbowWave { speed: 0.3, angle: 0.0, wavelength: 1.0 },
        )],
        assignments: Vec::new(),
        dial: actions::DialConfig::default(),
    }
}

pub fn profile_file_name(name: &str) -> String {
    let mut stem = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
            stem.push(c);
        } else {
            let mut bytes = [0u8; 4];
            for byte in c.encode_utf8(&mut bytes).bytes() {
                stem.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    format!("{stem}.toml")
}

#[derive(Debug, Clone)]
pub struct ConfigDir {
    root: PathBuf,
}

impl ConfigDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_location() -> Option<Self> {
        directories::ProjectDirs::from("", "", "k100").map(|dirs| Self::new(dirs.config_dir().to_path_buf()))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join(PROFILES_DIR)
    }

    pub fn sdk_trace_path(&self) -> PathBuf {
        self.logs_dir().join(SDK_TRACE_FILE)
    }

    pub fn standin_exe(&self) -> PathBuf {
        self.root.join(STANDIN_DIR).join(crate::standin::EXE_NAME)
    }

    pub fn only_generated_files_changed(&self, paths: &[PathBuf]) -> bool {
        let generated = [self.logs_dir(), self.root.join(STANDIN_DIR)];
        !paths.is_empty() && paths.iter().all(|path| generated.iter().any(|dir| path.starts_with(dir)))
    }

    fn logs_dir(&self) -> PathBuf {
        self.root.join(LOGS_DIR)
    }

    pub fn load_settings(&self) -> Result<Settings, String> {
        let path = self.root.join(SETTINGS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_atomically(&self.root.join(SETTINGS_FILE), &to_toml(settings)?)
    }

    pub fn load_profiles(&self) -> (Vec<Profile>, Vec<String>) {
        let (profiles, errors, _failed) = self.load_profile_set();
        (profiles, errors)
    }

    pub fn load_profile_set(&self) -> (Vec<Profile>, Vec<String>, Vec<String>) {
        let mut profiles: Vec<Profile> = Vec::new();
        let mut errors = Vec::new();
        let mut failed = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.profiles_dir()) else { return (profiles, errors, failed) };
        let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths.into_iter().filter(|p| p.extension().is_some_and(|ext| ext == "toml")) {
            let parsed = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| toml::from_str::<Profile>(&text).map_err(|error| error.to_string()))
                .and_then(|profile| profile.validate().map(|()| profile));
            let expected = parsed.as_ref().ok().map(|profile| profile_file_name(&profile.name));
            let actual = path.file_name().map(|name| name.to_string_lossy().into_owned());
            match parsed {
                Ok(profile) if expected != actual => errors.push(format!(
                    "{}: profile {:?} must be stored as {}",
                    path.display(),
                    profile.name,
                    expected.unwrap_or_default()
                )),
                Ok(profile) if profiles.iter().any(|p| p.name == profile.name) => {
                    errors.push(format!("{}: duplicate profile name {:?}", path.display(), profile.name));
                }
                Ok(profile) => profiles.push(profile),
                Err(error) => {
                    errors.push(format!("{}: {error}", path.display()));
                    failed.extend(path.file_name().map(|name| name.to_string_lossy().into_owned()));
                }
            }
        }
        (profiles, errors, failed)
    }

    pub fn save_profile(&self, profile: &Profile) -> io::Result<()> {
        write_atomically(&self.profiles_dir().join(profile_file_name(&profile.name)), &to_toml(profile)?)
    }

    pub fn delete_profile(&self, name: &str) -> io::Result<()> {
        std::fs::remove_file(self.profiles_dir().join(profile_file_name(name)))
    }

    pub fn ensure_default_profile(&self) -> io::Result<()> {
        let has_profile_files = std::fs::read_dir(self.profiles_dir())
            .map(|entries| entries.filter_map(Result::ok).any(|e| e.path().extension().is_some_and(|x| x == "toml")))
            .unwrap_or(false);
        if !has_profile_files {
            self.save_profile(&default_profile())?;
        }
        Ok(())
    }
}

fn to_toml<T: Serialize>(value: &T) -> io::Result<String> {
    toml::to_string_pretty(value).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, contents)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_actions_example_is_a_valid_profile() {
        let profile: Profile = toml::from_str(include_str!("../../../examples/profiles/actions.toml")).unwrap();
        profile.validate().unwrap();
        assert!(!profile.assignments.is_empty());
    }

    #[test]
    fn winlock_and_dial_ring_settings_parse() {
        let settings: Settings =
            toml::from_str("winlock_disables = [\"win\", \"alt_f4\"]\ndial_ring = \"continuous\"").unwrap();
        let mask: u32 = settings.winlock_disables.unwrap().iter().map(|s| s.bit()).sum();
        assert_eq!(mask, 5);
        assert_eq!(settings.dial_ring, DialRing::Continuous);
        assert_eq!(Settings::default().winlock_disables, None);
    }

    #[test]
    fn the_sdk_trace_lives_under_logs_and_log_writes_are_not_config_changes() {
        let config = ConfigDir::new(PathBuf::from("/config"));
        assert_eq!(config.sdk_trace_path(), Path::new("/config/logs/sdk-trace.jsonl"));
        assert!(config.only_generated_files_changed(&[config.sdk_trace_path(), PathBuf::from("/config/logs")]));
        assert!(
            !config.only_generated_files_changed(&[config.sdk_trace_path(), PathBuf::from("/config/settings.toml")])
        );
        assert!(!config.only_generated_files_changed(&[PathBuf::from("/config/logsettings.toml")]));
        assert!(!config.only_generated_files_changed(&[]));
    }

    #[test]
    fn the_standin_lives_in_its_own_folder_and_its_writes_are_not_config_changes() {
        let config = ConfigDir::new(PathBuf::from("/config"));
        assert_eq!(config.standin_exe(), Path::new("/config/icue-standin/iCUE.exe"));
        assert!(config.only_generated_files_changed(&[config.standin_exe(), PathBuf::from("/config/icue-standin")]));
        assert!(!config.only_generated_files_changed(&[config.standin_exe(), PathBuf::from("/config/settings.toml")]));
    }

    fn dir() -> (tempfile::TempDir, ConfigDir) {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        (temp, config)
    }

    #[test]
    fn missing_settings_mean_defaults() {
        let (_temp, config) = dir();
        assert_eq!(config.load_settings().unwrap(), Settings::default());
    }

    #[test]
    fn settings_round_trip() {
        let (_temp, config) = dir();
        let settings = Settings { polling_rate_hz: Some(2000), ..Settings::default() };
        config.save_settings(&settings).unwrap();
        assert_eq!(config.load_settings().unwrap(), settings);
    }

    #[test]
    fn creates_a_default_profile_only_when_none_exist() {
        let (_temp, config) = dir();
        config.ensure_default_profile().unwrap();
        let (profiles, errors) = config.load_profiles();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "default");
        config.save_profile(&Profile { name: "Games & Stuff".into(), ..default_profile() }).unwrap();
        config.ensure_default_profile().unwrap();
        assert_eq!(config.load_profiles().0.len(), 2);
        assert!(config.profiles_dir().join("%47ames%20%26%20%53tuff.toml").exists());
    }

    #[test]
    fn bad_files_are_reported_and_skipped() {
        let (_temp, config) = dir();
        config.save_profile(&default_profile()).unwrap();
        std::fs::write(config.profiles_dir().join("broken.toml"), "name = ").unwrap();
        std::fs::write(config.profiles_dir().join("copy.toml"), "name = \"default\"\n").unwrap();
        std::fs::write(config.profiles_dir().join("notes.txt"), "ignored").unwrap();
        let (profiles, errors) = config.load_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("broken.toml")));
        assert!(errors.iter().any(|e| e.contains("copy.toml") && e.contains("must be stored as default.toml")));
    }

    #[test]
    fn delete_removes_the_file() {
        let (_temp, config) = dir();
        config.save_profile(&Profile { name: "temp".into(), ..default_profile() }).unwrap();
        config.delete_profile("temp").unwrap();
        assert!(config.load_profiles().0.is_empty());
    }

    #[test]
    fn ignore_conflicts_defaults_to_empty_and_parses() {
        assert!(Settings::default().ignore_conflicts.is_empty());
        let settings: Settings = toml::from_str("ignore_conflicts = [\"OpenRGB.exe\"]").unwrap();
        assert_eq!(settings.ignore_conflicts, ["OpenRGB.exe"]);
    }

    #[test]
    fn file_names_are_reversible_and_distinct() {
        assert_eq!(profile_file_name("default"), "default.toml");
        assert_eq!(profile_file_name("my-games_2"), "my-games_2.toml");
        let names = ["A b", "a-b", "Games", "games", "\u{65e5}\u{672c}", "\u{4e2d}\u{6587}", "a%20b"];
        let files: std::collections::HashSet<String> =
            names.iter().map(|n| profile_file_name(n).to_lowercase()).collect();
        assert_eq!(files.len(), names.len(), "{files:?}");
    }

    #[test]
    fn files_whose_name_does_not_match_the_profile_are_reported() {
        let (_temp, config) = dir();
        std::fs::create_dir_all(config.profiles_dir()).unwrap();
        std::fs::write(config.profiles_dir().join("Demo.toml"), "name = \"demo\"\n").unwrap();
        let (profiles, errors) = config.load_profiles();
        assert!(profiles.is_empty());
        assert!(errors[0].contains("demo.toml"), "{errors:?}");
    }

    #[test]
    fn a_broken_default_is_not_overwritten() {
        let (_temp, config) = dir();
        std::fs::create_dir_all(config.profiles_dir()).unwrap();
        std::fs::write(config.profiles_dir().join("default.toml"), "name = ").unwrap();
        config.ensure_default_profile().unwrap();
        assert_eq!(std::fs::read_to_string(config.profiles_dir().join("default.toml")).unwrap(), "name = ");
    }

    #[test]
    fn brightness_lives_in_settings() {
        assert_eq!(Settings::default().brightness, 100);
        let settings: Settings = toml::from_str("brightness = 35").unwrap();
        assert_eq!(settings.brightness, 35);
    }

    #[test]
    fn profiles_with_invalid_assignments_are_rejected_like_parse_errors() {
        let (_temp, config) = dir();
        std::fs::create_dir_all(config.profiles_dir()).unwrap();
        let text = "name = \"bad\"\n[[assignments]]\nkey = \"dial\"\naction = { type = \"next_profile\" }\n";
        std::fs::write(config.profiles_dir().join("bad.toml"), text).unwrap();
        let (profiles, errors, failed) = config.load_profile_set();
        assert!(profiles.is_empty());
        assert!(errors[0].contains("dial cannot be reassigned"), "{errors:?}");
        assert_eq!(failed, ["bad.toml"]);
    }
}
