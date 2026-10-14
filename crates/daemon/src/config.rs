use std::io;
use std::path::{Path, PathBuf};

use engine::{EffectConfig, LayerConfig, NamedZone, Zone};
pub use ipc::Profile;
use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = "settings.toml";
const PROFILES_DIR: &str = "profiles";

fn version_one() -> u32 {
    1
}

fn default_profile_name() -> String {
    "default".into()
}

fn sixty() -> u32 {
    60
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "version_one")]
    pub version: u32,
    #[serde(default = "default_profile_name")]
    pub default_profile: String,
    #[serde(default = "sixty")]
    pub fps: u32,
    #[serde(default)]
    pub polling_rate_hz: Option<u32>,
    #[serde(default)]
    pub ignore_conflicts: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            default_profile: default_profile_name(),
            fps: 60,
            polling_rate_hz: None,
            ignore_conflicts: Vec::new(),
        }
    }
}

pub fn default_profile() -> Profile {
    Profile {
        version: 1,
        name: default_profile_name(),
        brightness: 100,
        apps: Vec::new(),
        layers: vec![LayerConfig::new(
            "rainbow",
            Zone::Named(NamedZone::All),
            EffectConfig::RainbowWave { speed: 0.3, angle: 0.0, wavelength: 1.0 },
        )],
    }
}

pub fn profile_file_name(name: &str) -> String {
    let stem: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
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
        let mut profiles: Vec<Profile> = Vec::new();
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.profiles_dir()) else { return (profiles, errors) };
        let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths.into_iter().filter(|p| p.extension().is_some_and(|ext| ext == "toml")) {
            let parsed = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| toml::from_str::<Profile>(&text).map_err(|error| error.to_string()));
            match parsed {
                Ok(profile) if profiles.iter().any(|p| p.name == profile.name) => {
                    errors.push(format!("{}: duplicate profile name {:?}", path.display(), profile.name));
                }
                Ok(profile) => profiles.push(profile),
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }
        (profiles, errors)
    }

    pub fn save_profile(&self, profile: &Profile) -> io::Result<()> {
        write_atomically(&self.profiles_dir().join(profile_file_name(&profile.name)), &to_toml(profile)?)
    }

    pub fn delete_profile(&self, name: &str) -> io::Result<()> {
        std::fs::remove_file(self.profiles_dir().join(profile_file_name(name)))
    }

    pub fn ensure_default_profile(&self) -> io::Result<()> {
        if self.load_profiles().0.is_empty() {
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
        assert!(config.profiles_dir().join("games---stuff.toml").exists());
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
        assert!(errors.iter().any(|e| e.contains("duplicate")));
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
}
