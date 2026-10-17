use actions::Key;

use crate::convert::KeyAction;
use crate::{
    AssignmentEntry, AssignmentMap, CommonLighting, DialModeEntry, DialModes, LightingEntry, LightingsList,
    ProfileRoot, START_WITH_PROFILE, Section, StoredFile, StoredKeyMappings,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardTree {
    pub name: String,
    pub effects: Vec<CommonLighting>,
    pub actions: Vec<KeyAction>,
    pub key_mappings: Vec<u8>,
    pub dial_modes: Vec<DialModeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePlan {
    pub label: String,
    pub bytes: usize,
}

struct PlannedFile {
    id: u16,
    label: String,
    file: StoredFile,
}

impl OnboardTree {
    const FIXED_FILES: usize = 4;

    pub fn file_count(&self) -> usize {
        let dial_file = usize::from(!self.dial_modes.is_empty());
        self.actions.len() + self.effects.len() + dial_file + Self::FIXED_FILES
    }

    /// Ids are used in file order: action files, assignment map, key mappings, effects, lightings list, dial modes if any, root.
    pub fn encode(&self, ids: &[u16]) -> Vec<(u16, Vec<u8>)> {
        self.plan(ids).into_iter().map(|planned| (planned.id, encoded(&planned.file))).collect()
    }

    pub fn manifest(&self) -> Vec<FilePlan> {
        let placeholder_ids = vec![0; self.file_count()];
        self.plan(&placeholder_ids)
            .into_iter()
            .map(|planned| FilePlan { label: planned.label, bytes: encoded(&planned.file).len() })
            .collect()
    }

    fn plan(&self, ids: &[u16]) -> Vec<PlannedFile> {
        assert_eq!(ids.len(), self.file_count(), "the tree needs {} file ids, got {}", self.file_count(), ids.len());
        let mut ids = ids.iter().copied();
        let mut next_id = || ids.next().expect("the id count was checked");
        let mut files = Vec::with_capacity(self.file_count());

        let mut assignments = Vec::new();
        for action in &self.actions {
            let id = next_id();
            assignments.push(AssignmentEntry { key: action.key, modifier: 0, action: id, execution: action.execution });
            let label = format!("action {} ({})", key_name(action.key), action.action.kind());
            files.push(PlannedFile { id, label, file: action.action.stored() });
        }

        let assignment_map = next_id();
        files.push(PlannedFile {
            id: assignment_map,
            label: "assignment map".into(),
            file: StoredFile::AssignmentMap(AssignmentMap { entries: assignments }),
        });

        let key_mappings = next_id();
        files.push(PlannedFile {
            id: key_mappings,
            label: "key mappings".into(),
            file: StoredFile::KeyMappings(StoredKeyMappings { bytes: self.key_mappings.clone() }),
        });

        let mut lightings = Vec::new();
        for (index, effect) in self.effects.iter().enumerate() {
            let id = next_id();
            lightings.push(LightingEntry { triggers: START_WITH_PROFILE, reserved: 0, file: id });
            files.push(PlannedFile {
                id,
                label: format!("effect {} ({:?})", index + 1, effect.kind),
                file: StoredFile::Lighting(effect.clone()),
            });
        }

        let lightings_list = next_id();
        files.push(PlannedFile {
            id: lightings_list,
            label: "lightings list".into(),
            file: StoredFile::LightingsList(LightingsList { entries: lightings }),
        });

        let dial_modes = (!self.dial_modes.is_empty()).then(|| {
            let id = next_id();
            files.push(PlannedFile {
                id,
                label: "dial modes".into(),
                file: StoredFile::DialModes(DialModes { modes: self.dial_modes.clone() }),
            });
            id
        });

        let mut root = ProfileRoot::new(&self.name);
        root.set(Section::AssignmentMap, assignment_map);
        root.set(Section::KeyMappings, key_mappings);
        root.set(Section::Lightings, lightings_list);
        if let Some(id) = dial_modes {
            root.set(Section::DialModes, id);
        }
        files.push(PlannedFile { id: next_id(), label: "root".into(), file: StoredFile::Root(root) });
        files
    }
}

fn encoded(file: &StoredFile) -> Vec<u8> {
    file.encode().expect("every planned file can be encoded")
}

fn key_name(index: u8) -> String {
    Key::from_index(index).map_or_else(|| format!("#{index}"), |key| key.name().to_owned())
}

#[cfg(test)]
mod tests {
    use k100::Rgb;

    use super::*;
    use crate::convert::{ActionFile, KeyAction};
    use crate::{
        AssignmentEntry, AssignmentMap, CommonLighting, DialModeEntry, KEY_COUNT, LightingEntry, LightingsList,
        ProfileRoot, RemapAction, RemapTarget, START_WITH_PROFILE, Section, execution,
    };

    fn sample_tree(dial: bool) -> OnboardTree {
        let remap = RemapAction::new(RemapTarget::Keyboard(vec![0x29]));
        OnboardTree {
            name: "t".into(),
            effects: vec![CommonLighting::static_color(Rgb::new(255, 0, 0), vec![4])],
            actions: vec![KeyAction {
                key: 57,
                action: ActionFile::Remap(remap),
                execution: execution::REMAP_WHILE_PRESSED,
            }],
            key_mappings: vec![0x01; KEY_COUNT],
            dial_modes: if dial { vec![DialModeEntry::new(0x01, Rgb::new(0, 0xBF, 0xFF))] } else { vec![] },
        }
    }

    #[test]
    fn files_get_ids_in_icue_order_with_the_root_last() {
        let tree = sample_tree(true);
        let ids: Vec<u16> = (0x6D60..0x6D67).collect();
        assert_eq!(tree.file_count(), ids.len());
        let files = tree.encode(&ids);
        assert_eq!(files.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids);
        let root = ProfileRoot::decode(&files[6].1).unwrap();
        let sections = [Section::AssignmentMap, Section::KeyMappings, Section::Lightings, Section::DialModes];
        assert_eq!(sections.map(|section| root.section(section)), [0x6D61, 0x6D62, 0x6D64, 0x6D65]);
        assert_eq!((root.section(Section::Archive), root.section(Section::PropertyMap), root.unknown), (0, 0, 0));
        let map = AssignmentMap::decode(&files[1].1).unwrap();
        assert_eq!(map.entries, [AssignmentEntry { key: 57, modifier: 0, action: 0x6D60, execution: 0x61 }]);
        let list = LightingsList::decode(&files[4].1).unwrap();
        assert_eq!(list.entries, [LightingEntry { triggers: START_WITH_PROFILE, reserved: 0, file: 0x6D63 }]);
    }

    #[test]
    fn a_tree_without_dial_modes_has_no_dial_file() {
        let tree = sample_tree(false);
        assert_eq!(tree.file_count(), 6);
        let files = tree.encode(&(0x6D60..0x6D66).collect::<Vec<_>>());
        assert_eq!(ProfileRoot::decode(&files[5].1).unwrap().section(Section::DialModes), 0);
    }

    #[test]
    fn the_manifest_names_every_file_with_its_size() {
        let manifest = sample_tree(true).manifest();
        let labels: Vec<&str> = manifest.iter().map(|file| file.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "action caps_lock (remap)",
                "assignment map",
                "key mappings",
                "effect 1 (StaticColor)",
                "lightings list",
                "dial modes",
                "root"
            ]
        );
        assert_eq!(manifest.iter().map(|file| file.bytes).collect::<Vec<_>>(), [10, 12, 140, 13, 8, 10, 42]);
    }

    #[test]
    #[should_panic(expected = "needs 7 file ids")]
    fn encode_panics_when_the_id_count_is_wrong() {
        sample_tree(true).encode(&[0x6D60]);
    }
}
