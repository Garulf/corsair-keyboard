use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello { version: u32 },
    GetState,
    ListProfiles,
    GetProfile { name: String },
    SaveProfile { profile: Profile },
    DeleteProfile { name: String },
    SetActiveProfile { name: String },
    SetBrightness { percent: u8 },
    SetPollingRate { hz: u32 },
    Release,
    Reacquire,
    Subscribe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Hello { version: u32 },
    State { state: DaemonState },
    Profiles { names: Vec<String> },
    Profile { profile: Profile },
    Done,
    Error { message: String },
    Event { event: Event },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    DeviceState { state: DeviceState },
    ProfileChanged { name: String },
    Key { led: u8, pressed: bool },
    Dial { clockwise: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceState {
    Disconnected,
    Connected { model: String, firmware: String },
    Blocked { processes: Vec<String> },
    Released,
    Paused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonState {
    pub device: DeviceState,
    pub active_profile: String,
    pub profiles: Vec<String>,
    pub brightness: u8,
    pub polling_rate_hz: Option<u32>,
    pub config_errors: Vec<String>,
}

fn version_one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default = "version_one")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub layers: Vec<engine::LayerConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignments: Vec<actions::Assignment>,
    #[serde(default, skip_serializing_if = "actions::DialConfig::is_default")]
    pub dial: actions::DialConfig,
}

impl Profile {
    pub fn validate(&self) -> Result<(), String> {
        actions::validate(&self.assignments, &self.dial)?;
        let dial_actions = self.dial.modes.iter().flat_map(|mode| mode.actions());
        for action in self.assignments.iter().map(|assignment| &assignment.action).chain(dial_actions) {
            if let actions::Action::LightingTrigger { layer, .. } = action
                && !self.layers.iter().any(|existing| existing.name == *layer)
            {
                return Err(format!("lighting trigger names the unknown layer {layer:?}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_tagged_snake_case() {
        let json = serde_json::to_string(&Request::SetActiveProfile { name: "games".into() }).unwrap();
        assert_eq!(json, r#"{"type":"set_active_profile","name":"games"}"#);
        let state = DeviceState::Blocked { processes: vec!["iCUE.exe".into()] };
        assert_eq!(serde_json::to_string(&state).unwrap(), r#"{"state":"blocked","processes":["iCUE.exe"]}"#);
    }

    #[test]
    fn profile_defaults_fill_missing_fields() {
        let profile: Profile = serde_json::from_str(r#"{"name":"plain"}"#).unwrap();
        assert_eq!(profile.version, 1);
        assert!(profile.layers.is_empty() && profile.apps.is_empty());
    }

    #[test]
    fn profile_typos_are_rejected() {
        let error = serde_json::from_str::<Profile>(r#"{"name":"plain","brightnes":5}"#).unwrap_err();
        assert!(error.to_string().contains("brightnes"), "{error}");
    }

    #[test]
    fn brightness_is_not_a_profile_field() {
        assert!(serde_json::from_str::<Profile>(r#"{"name":"plain","brightness":40}"#).is_err());
    }

    #[test]
    fn profiles_carry_assignments_and_validate_trigger_layers() {
        let text = r##"
name = "work"

[[layers]]
name = "alert"
zone = "keys"
enabled = false
effect = { type = "solid", color = "#ff0000" }

[[assignments]]
key = "g1"
action = { type = "lighting_trigger", layer = "alert" }
"##;
        let mut profile: Profile = toml::from_str(text).unwrap();
        profile.validate().unwrap();
        assert!(profile.dial.is_default());
        profile.layers.clear();
        assert!(profile.validate().unwrap_err().contains("alert"));
        let remap_to_g2 = "name = \"x\"\n[[assignments]]\nkey = \"g1\"\naction = { type = \"remap\", key = \"g2\" }\n";
        assert!(toml::from_str::<Profile>(remap_to_g2).unwrap().validate().is_err());
    }
}
