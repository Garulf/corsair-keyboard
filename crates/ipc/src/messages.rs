use serde::{Deserialize, Serialize};

use crate::settings::{Settings, SettingsPatch};

pub const PROTOCOL_VERSION: u32 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
    },
    GetState,
    ListProfiles,
    GetProfile {
        name: String,
    },
    SaveProfile {
        profile: Profile,
    },
    DeleteProfile {
        name: String,
    },
    SetActiveProfile {
        name: String,
    },
    SetBrightness {
        percent: u8,
    },
    SetPollingRate {
        hz: u32,
    },
    SetWinLock {
        enabled: bool,
    },
    SetDebounce {
        enabled: bool,
        timeout: u32,
    },
    GetSettings,
    SetSettings {
        patch: SettingsPatch,
    },
    StartRecording {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<u8>,
    },
    StopRecording,
    CancelRecording,
    SetAutostart {
        enabled: bool,
    },
    Release,
    Reacquire,
    Subscribe,
    SubscribeFrames,
    Onboard {
        request: OnboardRequest,
    },
    SdkBlock {
        exe: String,
        blocked: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OnboardRequest {
    List,
    Save {
        profile: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u32>,
    },
    Delete {
        slot: u32,
    },
    Activate {
        slot: u32,
    },
    Backup {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    Restore {
        from: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardSlot {
    pub index: u32,
    pub name: String,
    pub files: u32,
    pub bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Hello { version: u32 },
    State { state: DaemonState },
    Profiles { names: Vec<String> },
    Profile { profile: Profile },
    Settings { settings: Settings },
    Done,
    Error { message: String },
    Event { event: Event },
    OnboardSlots { slots: Vec<OnboardSlot>, active: Option<u32>, free_bytes: Option<u32> },
    OnboardDone { message: String, backup: Option<String>, notes: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    DeviceState { state: DeviceState },
    ProfileChanged { name: String },
    ProfileEdited { name: String },
    Key { led: u8, pressed: bool },
    Dial { clockwise: bool },
    WinLock { enabled: bool },
    DialMode { name: String },
    Recording { state: actions::RecorderState },
    MacroRecorded { key: String },
    Brightness { percent: u8 },
    Audio { state: AudioState },
    Sdk { state: SdkState },
    Frame { rgb: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceState {
    #[default]
    Disconnected,
    Connected {
        model: String,
        firmware: String,
    },
    Blocked {
        processes: Vec<String>,
    },
    Released,
    Paused {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AudioState {
    #[default]
    Off,
    Starting,
    Capturing {
        device: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SdkStatus {
    #[default]
    Unsupported,
    Off,
    Listening,
    InUse,
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdkAccess {
    Shared,
    ExclusiveLighting,
    ExclusiveKeys,
    Exclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkClient {
    pub exe: String,
    pub access: SdkAccess,
    pub priority: u8,
    pub intercepted: Vec<String>,
    pub refused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SdkState {
    pub status: SdkStatus,
    pub clients: Vec<SdkClient>,
    pub blocked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DaemonState {
    pub device: DeviceState,
    pub active_profile: String,
    pub profiles: Vec<String>,
    pub brightness: u8,
    pub polling_rate_hz: Option<u32>,
    #[serde(default)]
    pub max_polling_rate_hz: Option<u32>,
    pub config_errors: Vec<String>,
    #[serde(default)]
    pub winlock: Option<bool>,
    #[serde(default)]
    pub dial_mode: String,
    #[serde(default)]
    pub recording: actions::RecorderState,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub audio: AudioState,
    #[serde(default)]
    pub sdk: SdkState,
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
    fn new_events_serialize_with_their_payloads() {
        let event = Event::Recording { state: actions::RecorderState::Recording { key: 131 } };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"recording","state":{"state":"recording","key":131}}"#
        );
        let old_state = r#"{"device":{"state":"released"},"active_profile":"a","profiles":[],"brightness":5,"polling_rate_hz":null,"config_errors":[]}"#;
        let state: DaemonState = serde_json::from_str(old_state).unwrap();
        assert_eq!((state.winlock, state.dial_mode.as_str()), (None, ""));
    }

    #[test]
    fn daemon_state_without_audio_parses_as_off() {
        let old_state = r#"{"device":{"state":"released"},"active_profile":"a","profiles":[],"brightness":5,"polling_rate_hz":null,"config_errors":[]}"#;
        let state: DaemonState = serde_json::from_str(old_state).unwrap();
        assert_eq!(state.audio, AudioState::Off);
    }

    #[test]
    fn daemon_state_without_sdk_parses_as_unsupported() {
        let old_state = r#"{"device":{"state":"released"},"active_profile":"a","profiles":[],"brightness":5,"polling_rate_hz":null,"config_errors":[]}"#;
        let state: DaemonState = serde_json::from_str(old_state).unwrap();
        assert_eq!(state.sdk, SdkState::default());
        assert_eq!(state.sdk.status, SdkStatus::Unsupported);
    }

    #[test]
    fn sdk_state_round_trips() {
        let client = SdkClient {
            exe: "StreamDeck.exe".into(),
            access: SdkAccess::ExclusiveKeys,
            priority: 128,
            intercepted: vec!["G1".into()],
            refused: false,
        };
        let listening = SdkState { status: SdkStatus::Listening, clients: vec![client], blocked: vec!["x.exe".into()] };
        let json = serde_json::to_string(&listening).unwrap();
        assert_eq!(
            json,
            r#"{"status":{"status":"listening"},"clients":[{"exe":"StreamDeck.exe","access":"exclusive_keys","priority":128,"intercepted":["G1"],"refused":false}],"blocked":["x.exe"]}"#
        );
        assert_eq!(serde_json::from_str::<SdkState>(&json).unwrap(), listening);
        for status in [SdkStatus::InUse, SdkStatus::Off, SdkStatus::Failed { reason: "denied".into() }] {
            let state = SdkState { status, ..SdkState::default() };
            assert_eq!(serde_json::from_str::<SdkState>(&serde_json::to_string(&state).unwrap()).unwrap(), state);
        }
        let failed = serde_json::to_string(&SdkStatus::Failed { reason: "denied".into() }).unwrap();
        assert_eq!(failed, r#"{"status":"failed","reason":"denied"}"#);
        let event = Event::Sdk { state: SdkState::default() };
        assert_eq!(serde_json::from_str::<Event>(&serde_json::to_string(&event).unwrap()).unwrap(), event);
        let block = Request::SdkBlock { exe: "x.exe".into(), blocked: true };
        assert_eq!(serde_json::to_string(&block).unwrap(), r#"{"type":"sdk_block","exe":"x.exe","blocked":true}"#);
    }

    #[test]
    fn audio_states_round_trip_with_their_payloads() {
        let failed = AudioState::Failed { reason: "no sink".into() };
        let json = serde_json::to_string(&failed).unwrap();
        assert_eq!(json, r#"{"state":"failed","reason":"no sink"}"#);
        assert_eq!(serde_json::from_str::<AudioState>(&json).unwrap(), failed);
        let event = Event::Audio { state: AudioState::Capturing { device: "Speakers".into() } };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"type":"audio","state":{"state":"capturing","device":"Speakers"}}"#);
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }

    #[test]
    fn profile_edited_events_serialize_with_the_profile_name() {
        let event = Event::ProfileEdited { name: "games".into() };
        assert_eq!(serde_json::to_string(&event).unwrap(), r#"{"type":"profile_edited","name":"games"}"#);
    }

    #[test]
    fn settings_and_recording_requests_use_snake_case_tags() {
        let debounce = serde_json::to_string(&Request::SetDebounce { enabled: true, timeout: 30 }).unwrap();
        assert_eq!(debounce, r#"{"type":"set_debounce","enabled":true,"timeout":30}"#);
        let patch = SettingsPatch { auto_switch: Some(false), ..SettingsPatch::default() };
        let set = serde_json::to_string(&Request::SetSettings { patch }).unwrap();
        assert_eq!(set, r#"{"type":"set_settings","patch":{"auto_switch":false}}"#);
        let start = Request::StartRecording { key: None };
        assert_eq!(serde_json::to_string(&start).unwrap(), r#"{"type":"start_recording"}"#);
        assert_eq!(serde_json::to_string(&Request::GetSettings).unwrap(), r#"{"type":"get_settings"}"#);
    }

    #[test]
    fn recording_can_target_a_key_and_be_stopped_from_a_client() {
        let bare: Request = serde_json::from_str(r#"{"type":"start_recording"}"#).unwrap();
        assert_eq!(bare, Request::StartRecording { key: None });
        let keyed = Request::StartRecording { key: Some(132) };
        let json = serde_json::to_string(&keyed).unwrap();
        assert_eq!(json, r#"{"type":"start_recording","key":132}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), keyed);
        let stop = serde_json::to_string(&Request::StopRecording).unwrap();
        assert_eq!(stop, r#"{"type":"stop_recording"}"#);
        assert_eq!(serde_json::from_str::<Request>(&stop).unwrap(), Request::StopRecording);
    }

    #[test]
    fn onboard_requests_nest_their_operation() {
        let save = Request::Onboard { request: OnboardRequest::Save { profile: "games".into(), slot: None } };
        assert_eq!(
            serde_json::to_string(&save).unwrap(),
            r#"{"type":"onboard","request":{"op":"save","profile":"games"}}"#
        );
        let restore = Request::Onboard { request: OnboardRequest::Restore { from: "/b/1".into() } };
        assert_eq!(serde_json::from_str::<Request>(&serde_json::to_string(&restore).unwrap()).unwrap(), restore);
        let slot = OnboardSlot { index: 0, name: "HW Profile 2".into(), files: 16, bytes: 570 };
        let slots = Reply::OnboardSlots { slots: vec![slot], active: Some(0), free_bytes: None };
        assert_eq!(serde_json::from_str::<Reply>(&serde_json::to_string(&slots).unwrap()).unwrap(), slots);
    }

    #[test]
    fn onboard_replies_and_optional_fields_round_trip() {
        let backup = Request::Onboard { request: OnboardRequest::Backup { to: None } };
        assert_eq!(serde_json::to_string(&backup).unwrap(), r#"{"type":"onboard","request":{"op":"backup"}}"#);
        let overwrite: Request =
            serde_json::from_str(r#"{"type":"onboard","request":{"op":"save","profile":"a","slot":2}}"#).unwrap();
        assert_eq!(
            overwrite,
            Request::Onboard { request: OnboardRequest::Save { profile: "a".into(), slot: Some(2) } }
        );
        let done = Reply::OnboardDone { message: "deleted slot 1".into(), backup: Some("/b".into()), notes: vec![] };
        assert_eq!(serde_json::from_str::<Reply>(&serde_json::to_string(&done).unwrap()).unwrap(), done);
        assert_eq!(PROTOCOL_VERSION, 5);
    }

    #[test]
    fn frame_events_and_requests_serialize() {
        assert_eq!(serde_json::to_string(&Request::SubscribeFrames).unwrap(), r#"{"type":"subscribe_frames"}"#);
        let event = Event::Frame { rgb: "00ff00".into() };
        assert_eq!(serde_json::to_string(&event).unwrap(), r#"{"type":"frame","rgb":"00ff00"}"#);
    }

    #[test]
    fn daemon_state_defaults_to_a_disconnected_blank() {
        let state = DaemonState::default();
        assert_eq!(state.device, DeviceState::Disconnected);
        assert!(state.profiles.is_empty() && state.max_polling_rate_hz.is_none());
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
