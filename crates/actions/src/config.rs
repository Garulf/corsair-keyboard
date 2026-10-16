use k100::{KeyMappings, Mapping, Rgb, led};
use serde::{Deserialize, Serialize};

use crate::keys::Key;

const PROTECTED_KEYS: [u8; 2] = [led::DIAL_KEY, led::WIN_LOCK];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Once,
    WhileHeld,
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum MacroStep {
    Down(Key),
    Up(Key),
    Tap(Key),
    #[serde(rename = "delay_ms")]
    Delay(u32),
    Text(String),
    MouseDown(MouseButton),
    MouseUp(MouseButton),
    Click(MouseButton),
    Wheel(i32),
    Hwheel(i32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Remap {
        key: Key,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        modifiers: Vec<Key>,
    },
    Macro {
        steps: Vec<MacroStep>,
        #[serde(default)]
        mode: RepeatMode,
    },
    Text {
        text: String,
    },
    Launch {
        program: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    OpenUrl {
        url: String,
    },
    Mouse {
        button: MouseButton,
    },
    Disable,
    SwitchProfile {
        name: String,
    },
    NextProfile,
    PreviousProfile,
    LightingTrigger {
        layer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u32>,
    },
    BrightnessUp,
    BrightnessDown,
    SetBrightness {
        percent: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassThrough {
    Fn,
    Ctrl,
    Alt,
    Shift,
}

impl PassThrough {
    pub fn mapping(self) -> Mapping {
        match self {
            Self::Fn => Mapping::WITH_FN,
            Self::Ctrl => Mapping::WITH_CTRL,
            Self::Alt => Mapping::WITH_ALT,
            Self::Shift => Mapping::WITH_SHIFT,
        }
    }

    pub fn held_by(self) -> &'static [u8] {
        match self {
            Self::Fn => &[led::FN],
            Self::Ctrl => &[105, 109],
            Self::Alt => &[107, 111],
            Self::Shift => &[106, 110],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub key: Key,
    pub action: Action,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_through: Vec<PassThrough>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialModeKind {
    Brightness,
    MacroRecording,
    MediaJogging,
    TrackSelector,
    VerticalScroll,
    HorizontalScroll,
    Zoom,
    SwitchApps,
    Custom,
}

impl DialModeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Brightness => "brightness",
            Self::MacroRecording => "macro_recording",
            Self::MediaJogging => "media_jogging",
            Self::TrackSelector => "track_selector",
            Self::VerticalScroll => "vertical_scroll",
            Self::HorizontalScroll => "horizontal_scroll",
            Self::Zoom => "zoom",
            Self::SwitchApps => "switch_apps",
            Self::Custom => "custom",
        }
    }

    pub fn default_color(self) -> Rgb {
        match self {
            Self::Brightness => Rgb::new(0x00, 0xBF, 0xFF),
            Self::MacroRecording => Rgb::new(0xFF, 0x00, 0x00),
            Self::MediaJogging => Rgb::new(0x00, 0xFF, 0x00),
            Self::TrackSelector => Rgb::new(0xFF, 0xFF, 0xFF),
            Self::VerticalScroll => Rgb::new(0xFF, 0x80, 0x00),
            Self::HorizontalScroll => Rgb::new(0xFF, 0xFF, 0x00),
            Self::Zoom => Rgb::new(0x80, 0x00, 0x80),
            Self::SwitchApps => Rgb::new(0xBF, 0xFF, 0x00),
            Self::Custom => Rgb::new(0xFF, 0x00, 0xFF),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialMode {
    pub kind: DialModeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clockwise: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_clockwise: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<Action>,
}

impl DialMode {
    pub fn builtin(kind: DialModeKind) -> Self {
        Self { kind, name: None, color: None, clockwise: None, counter_clockwise: None, press: None }
    }

    pub fn color(&self) -> Rgb {
        self.color.unwrap_or(self.kind.default_color())
    }

    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.kind.label().to_owned())
    }

    pub fn actions(&self) -> impl Iterator<Item = &Action> {
        [&self.clockwise, &self.counter_clockwise, &self.press].into_iter().flatten()
    }
}

fn default_modes() -> Vec<DialMode> {
    [DialModeKind::Brightness, DialModeKind::MacroRecording, DialModeKind::MediaJogging, DialModeKind::TrackSelector]
        .into_iter()
        .map(DialMode::builtin)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialConfig {
    #[serde(default = "default_modes")]
    pub modes: Vec<DialMode>,
}

impl Default for DialConfig {
    fn default() -> Self {
        Self { modes: default_modes() }
    }
}

impl DialConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

pub fn validate(assignments: &[Assignment], dial: &DialConfig) -> Result<(), String> {
    let mut seen = Vec::new();
    for assignment in assignments {
        let key = assignment.key;
        let Some(index) = key.index() else {
            return Err(format!("{key} is not a key on the keyboard"));
        };
        if !key.is_on_k100() {
            return Err(format!("{key} is not on the K100"));
        }
        if PROTECTED_KEYS.contains(&index) {
            return Err(format!("{key} cannot be reassigned"));
        }
        if seen.contains(&index) {
            return Err(format!("{key} is assigned twice"));
        }
        seen.push(index);
        validate_action(&assignment.action).map_err(|error| format!("{key}: {error}"))?;
    }
    if dial.modes.is_empty() {
        return Err("the dial needs at least one mode".into());
    }
    for mode in &dial.modes {
        let has_actions = mode.actions().next().is_some();
        match (mode.kind == DialModeKind::Custom, has_actions) {
            (true, false) => {
                return Err("a custom dial mode needs clockwise, counter_clockwise or press".into());
            }
            (false, true) => {
                return Err(format!("the {} dial mode takes no actions; use kind = \"custom\"", mode.kind.label()));
            }
            _ => {}
        }
        for action in mode.actions() {
            validate_action(action).map_err(|error| format!("dial mode {}: {error}", mode.label()))?;
        }
    }
    Ok(())
}

fn validate_action(action: &Action) -> Result<(), String> {
    match action {
        Action::Remap { key, modifiers } => {
            sendable(*key)?;
            modifiers.iter().try_for_each(|modifier| sendable(*modifier))
        }
        Action::Macro { steps, .. } if steps.is_empty() => Err("the macro has no steps".into()),
        Action::Macro { steps, .. } => steps.iter().try_for_each(|step| match step {
            MacroStep::Down(key) | MacroStep::Up(key) | MacroStep::Tap(key) => sendable(*key),
            _ => Ok(()),
        }),
        Action::SetBrightness { percent } if *percent > 100 => Err("brightness must be 0 to 100".into()),
        _ => Ok(()),
    }
}

fn sendable(key: Key) -> Result<(), String> {
    key.output().map(drop).ok_or_else(|| format!("{key} cannot be sent to the computer"))
}

pub fn key_mappings(assignments: &[Assignment], key_count: usize) -> KeyMappings {
    let mut mappings = KeyMappings::all_normal(key_count);
    for assignment in assignments {
        if let Some(index) = assignment.key.index() {
            let mapping = assignment.pass_through.iter().fold(Mapping::NO_REPORT, |all, pass| all | pass.mapping());
            mappings.set(index, mapping);
        }
    }
    mappings
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn assign(name: &str, action: Action) -> Assignment {
        Assignment { key: key(name), action, pass_through: Vec::new() }
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Doc {
        #[serde(default)]
        assignments: Vec<Assignment>,
        #[serde(default)]
        dial: DialConfig,
    }

    const SAMPLE: &str = r##"
[[assignments]]
key = "g1"
action = { type = "macro", mode = "toggle", steps = [{ down = "left_ctrl" }, { tap = "c" }, { delay_ms = 20 }, { up = "left_ctrl" }] }

[[assignments]]
key = "caps_lock"
action = { type = "remap", key = "escape" }
pass_through = ["shift"]

[[assignments]]
key = "g2"
action = { type = "launch", program = "notepad.exe" }

[[dial.modes]]
kind = "brightness"

[[dial.modes]]
kind = "custom"
name = "volume"
color = "#123456"
clockwise = { type = "remap", key = "volume_up" }
counter_clockwise = { type = "remap", key = "volume_down" }
"##;

    #[test]
    fn parses_assignments_and_dial_modes() {
        let doc: Doc = toml::from_str(SAMPLE).unwrap();
        assert_eq!(doc.assignments.len(), 3);
        let steps = vec![
            MacroStep::Down(key("left_ctrl")),
            MacroStep::Tap(key("c")),
            MacroStep::Delay(20),
            MacroStep::Up(key("left_ctrl")),
        ];
        assert_eq!(doc.assignments[0].action, Action::Macro { steps, mode: RepeatMode::Toggle });
        assert_eq!(doc.assignments[1].pass_through, [PassThrough::Shift]);
        assert_eq!(doc.dial.modes[0].color(), Rgb::new(0x00, 0xBF, 0xFF));
        assert_eq!(doc.dial.modes[1].color(), Rgb::new(0x12, 0x34, 0x56));
        assert_eq!(doc.dial.modes[1].label(), "volume");
        assert_eq!(doc.dial.modes[0].label(), "brightness");
        validate(&doc.assignments, &doc.dial).unwrap();
    }

    #[test]
    fn round_trips_through_toml() {
        let doc: Doc = toml::from_str(SAMPLE).unwrap();
        let text = toml::to_string(&doc).unwrap();
        assert_eq!(toml::from_str::<Doc>(&text).unwrap(), doc);
    }

    #[test]
    fn default_dial_matches_icue() {
        let labels: Vec<String> = DialConfig::default().modes.iter().map(DialMode::label).collect();
        assert_eq!(labels, ["brightness", "macro_recording", "media_jogging", "track_selector"]);
        assert!(DialConfig::default().is_default());
        assert_eq!(DialModeKind::MacroRecording.default_color(), Rgb::new(255, 0, 0));
        assert_eq!(DialModeKind::SwitchApps.default_color(), Rgb::new(0xBF, 0xFF, 0x00));
    }

    #[test]
    fn typos_are_rejected() {
        let typo = "[[assignments]]\nkey = \"g1\"\naction = { type = \"remap\", kye = \"a\" }\n";
        assert!(toml::from_str::<Doc>(typo).unwrap_err().to_string().contains("kye"));
        let step = "[[assignments]]\nkey = \"g1\"\naction = { type = \"macro\", steps = [{ dwn = \"a\" }] }\n";
        assert!(toml::from_str::<Doc>(step).is_err());
    }

    #[test]
    fn validation_explains_bad_assignments() {
        let dial = DialConfig::default();
        let remap = |target: &str| Action::Remap { key: key(target), modifiers: Vec::new() };
        let cases = [
            (vec![assign("g1", remap("g2"))], "cannot be sent"),
            (vec![assign("dial", remap("a"))], "cannot be reassigned"),
            (vec![assign("win_lock", remap("a"))], "cannot be reassigned"),
            (vec![assign("f13", remap("a"))], "not a key on the keyboard"),
            (vec![assign("mr", remap("a"))], "mr is not on the K100"),
            (vec![assign("brightness", remap("a"))], "brightness is not on the K100"),
            (vec![assign("g1", remap("a")), assign("g1", remap("b"))], "assigned twice"),
            (vec![assign("g1", Action::Macro { steps: Vec::new(), mode: RepeatMode::Once })], "no steps"),
            (vec![assign("g1", Action::SetBrightness { percent: 101 })], "0 to 100"),
        ];
        for (assignments, expected) in cases {
            let error = validate(&assignments, &dial).unwrap_err();
            assert!(error.contains(expected), "{error:?} should mention {expected:?}");
        }
    }

    #[test]
    fn validation_checks_dial_modes() {
        assert!(validate(&[], &DialConfig { modes: Vec::new() }).unwrap_err().contains("at least one"));
        let lonely = DialMode::builtin(DialModeKind::Custom);
        assert!(validate(&[], &DialConfig { modes: vec![lonely] }).unwrap_err().contains("custom"));
        let busy = DialMode { press: Some(Action::NextProfile), ..DialMode::builtin(DialModeKind::Zoom) };
        assert!(validate(&[], &DialConfig { modes: vec![busy] }).unwrap_err().contains("takes no actions"));
    }

    #[test]
    fn key_mappings_suppress_assigned_keys() {
        let mut caps = assign("caps_lock", Action::Disable);
        caps.pass_through = vec![PassThrough::Ctrl, PassThrough::Shift];
        let bytes = key_mappings(&[caps, assign("g1", Action::NextProfile)], 138).as_bytes().to_vec();
        assert_eq!(bytes.len(), 138);
        assert_eq!(bytes[57], 0x28);
        assert_eq!(bytes[131], 0x00);
        assert_eq!(bytes[4], 0x01);
    }
}
