use serde::{Deserialize, Serialize};

pub const MAX_DEBOUNCE_TIMEOUT: u32 = 300;

fn version_one() -> u32 {
    1
}

fn default_profile_name() -> String {
    "default".into()
}

fn enabled() -> bool {
    true
}

fn sixty() -> u32 {
    60
}

fn full_brightness() -> u8 {
    100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialRing {
    #[default]
    Temporary,
    Continuous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WinLockShortcut {
    Win,
    AltTab,
    AltF4,
    ShiftTab,
}

impl WinLockShortcut {
    pub const ALL: [Self; 4] = [Self::Win, Self::AltTab, Self::AltF4, Self::ShiftTab];

    pub fn bit(self) -> u32 {
        match self {
            Self::Win => 1,
            Self::AltTab => 2,
            Self::AltF4 => 4,
            Self::ShiftTab => 8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Win => "Windows key",
            Self::AltTab => "Alt+Tab",
            Self::AltF4 => "Alt+F4",
            Self::ShiftTab => "Shift+Tab",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Debounce {
    pub enabled: bool,
    pub timeout: u32,
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
    #[serde(default = "full_brightness")]
    pub brightness: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winlock_disables: Option<Vec<WinLockShortcut>>,
    #[serde(default)]
    pub dial_ring: DialRing,
    #[serde(default = "enabled")]
    pub auto_switch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce: Option<Debounce>,
    #[serde(default)]
    pub sdk: SdkSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkSettings {
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default = "enabled")]
    pub exclusive_control: bool,
    #[serde(default)]
    pub blocked: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default)]
    pub trace: bool,
}

impl Default for SdkSettings {
    fn default() -> Self {
        Self { enabled: true, exclusive_control: true, blocked: Vec::new(), device_id: None, trace: false }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            default_profile: default_profile_name(),
            fps: 60,
            polling_rate_hz: None,
            ignore_conflicts: Vec::new(),
            brightness: 100,
            winlock_disables: None,
            dial_ring: DialRing::default(),
            auto_switch: true,
            debounce: None,
            sdk: SdkSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winlock_disables: Option<Vec<WinLockShortcut>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_ring: Option<DialRing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_switch: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

impl SettingsPatch {
    pub fn apply(&self, settings: &mut Settings) {
        if let Some(shortcuts) = &self.winlock_disables {
            settings.winlock_disables = Some(shortcuts.clone());
        }
        if let Some(ring) = self.dial_ring {
            settings.dial_ring = ring;
        }
        if let Some(auto_switch) = self.auto_switch {
            settings.auto_switch = auto_switch;
        }
        if let Some(name) = &self.default_profile {
            settings.default_profile = name.clone();
        }
    }

    pub fn between(old: &Settings, new: &Settings) -> Self {
        Self {
            winlock_disables: (old.winlock_disables != new.winlock_disables)
                .then(|| new.winlock_disables.clone().unwrap_or_default()),
            dial_ring: (old.dial_ring != new.dial_ring).then_some(new.dial_ring),
            auto_switch: (old.auto_switch != new.auto_switch).then_some(new.auto_switch),
            default_profile: (old.default_profile != new.default_profile).then(|| new.default_profile.clone()),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_map_to_the_firmware_bits() {
        let mask: u32 = WinLockShortcut::ALL.iter().map(|s| s.bit()).sum();
        assert_eq!(mask, 15);
        assert_eq!(WinLockShortcut::AltF4.bit(), 4);
    }

    #[test]
    fn settings_files_without_debounce_still_parse() {
        let settings: Settings = toml::from_str("brightness = 35").unwrap();
        assert_eq!((settings.brightness, settings.debounce), (35, None));
        let with: Settings = toml::from_str("debounce = { enabled = true, timeout = 30 }").unwrap();
        assert_eq!(with.debounce, Some(Debounce { enabled: true, timeout: 30 }));
        assert!(toml::from_str::<Settings>("debounce = { enabled = true }").is_err());
    }

    #[test]
    fn settings_without_sdk_use_the_defaults_and_sdk_parses() {
        let settings: Settings = toml::from_str("brightness = 35").unwrap();
        assert_eq!(settings.sdk, SdkSettings::default());
        assert!(settings.sdk.enabled && settings.sdk.exclusive_control && !settings.sdk.trace);
        assert_eq!((settings.sdk.blocked.len(), settings.sdk.device_id.as_deref()), (0, None));
        let with: Settings = toml::from_str("[sdk]\nenabled = false\nblocked = [\"StreamDeck.exe\"]").unwrap();
        assert_eq!((with.sdk.enabled, with.sdk.blocked.as_slice()), (false, &["StreamDeck.exe".to_owned()][..]));
        assert!(toml::from_str::<Settings>("[sdk]\npopup = true").is_err());
        let text = toml::to_string(&Settings::default()).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), Settings::default());
        let identified = Settings {
            sdk: SdkSettings { device_id: Some("{x}".into()), trace: true, ..SdkSettings::default() },
            ..Settings::default()
        };
        let text = toml::to_string(&identified).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), identified);
    }

    #[test]
    fn a_patch_changes_only_the_fields_it_names() {
        let mut settings = Settings { brightness: 40, polling_rate_hz: Some(1000), ..Settings::default() };
        let patch = SettingsPatch {
            dial_ring: Some(DialRing::Continuous),
            winlock_disables: Some(vec![WinLockShortcut::Win]),
            ..SettingsPatch::default()
        };
        patch.apply(&mut settings);
        assert_eq!(settings.dial_ring, DialRing::Continuous);
        assert_eq!(settings.winlock_disables, Some(vec![WinLockShortcut::Win]));
        assert_eq!((settings.brightness, settings.polling_rate_hz, settings.auto_switch), (40, Some(1000), true));
    }

    #[test]
    fn between_names_exactly_the_differences() {
        let old = Settings::default();
        assert!(SettingsPatch::between(&old, &old).is_empty());
        let new = Settings { auto_switch: false, default_profile: "games".into(), ..Settings::default() };
        let patch = SettingsPatch::between(&old, &new);
        assert_eq!(patch.auto_switch, Some(false));
        assert_eq!(patch.default_profile.as_deref(), Some("games"));
        assert_eq!((patch.dial_ring, patch.winlock_disables), (None, None));
    }

    #[test]
    fn an_empty_patch_serializes_to_an_empty_object() {
        assert_eq!(serde_json::to_string(&SettingsPatch::default()).unwrap(), "{}");
    }
}
