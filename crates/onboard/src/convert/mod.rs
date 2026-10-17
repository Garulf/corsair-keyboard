mod actions;
mod lighting;

pub use actions::{ActionFile, KeyAction, convert_assignments, convert_dial};
pub use lighting::{MAX_EFFECTS, convert_layers};

use engine::Layout;
use ipc::Profile;

use crate::OnboardTree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    pub tree: OnboardTree,
    pub report: Report,
}

pub fn convert(profile: &Profile) -> Conversion {
    let mut report = Report::default();
    let effects = convert_layers(&profile.layers, &Layout::ansi(), &mut report);
    let (actions, key_mappings) = convert_assignments(&profile.assignments, &mut report);
    let dial_modes = convert_dial(&profile.dial, &mut report);
    let tree = OnboardTree { name: profile.name.clone(), effects, actions, key_mappings, dial_modes };
    Conversion { tree, report }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub skipped: Vec<String>,
    pub approximated: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.skipped.is_empty() && self.approximated.is_empty()
    }

    pub fn lines(&self) -> Vec<String> {
        let skipped = self.skipped.iter().map(|line| format!("skipped: {line}"));
        let approximated = self.approximated.iter().map(|line| format!("approximated: {line}"));
        skipped.chain(approximated).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_list_skipped_before_approximated() {
        let report = Report { skipped: vec!["a".into()], approximated: vec!["b".into()] };
        assert_eq!(report.lines(), ["skipped: a", "approximated: b"]);
        assert!(!report.is_empty());
        assert!(Report::default().is_empty());
    }

    #[test]
    fn the_actions_example_converts_with_a_report() {
        let profile: Profile = toml::from_str(include_str!("../../../../examples/profiles/actions.toml")).unwrap();
        let Conversion { tree, report } = convert(&profile);
        assert_eq!((tree.name.as_str(), tree.effects.len(), tree.file_count()), ("actions", 1, 10));
        assert_eq!(tree.actions.iter().map(|action| action.key).collect::<Vec<_>>(), [131, 132, 135, 57]);
        assert_eq!(tree.dial_modes.iter().map(|mode| mode.kind).collect::<Vec<_>>(), [0x01, 0x08, 0x04]);
        assert_eq!((tree.key_mappings[57], tree.key_mappings[133], tree.key_mappings[134]), (0x20, 0x01, 0x01));
        let lines = report.lines().join("\n");
        for expected in ["\"alert\"", "g3", "g4", "macro_recording"] {
            assert!(lines.contains(expected), "{expected}: {lines}");
        }
    }
}
