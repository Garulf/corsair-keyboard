use std::path::Path;
use std::time::SystemTime;

use bragi::FileId;
use ipc::{OnboardRequest, OnboardSlot, Profile, Reply};

use crate::{
    Backups, Changed, FileStore, OnboardError, ProfileRoot, ProfilesList, SlotSummary, Snapshot, activate, convert,
    delete, list, restore, save, snapshot,
};

pub fn execute<S: FileStore + ?Sized>(
    store: &mut S,
    request: &OnboardRequest,
    profiles: &dyn Fn(&str) -> Option<Profile>,
    backups: &Backups,
    now: SystemTime,
) -> Result<Reply, OnboardError> {
    match request {
        OnboardRequest::List => list_slots(store),
        OnboardRequest::Save { profile, slot } => {
            let profile = profiles(profile).ok_or_else(|| OnboardError::NoProfile(profile.clone()))?;
            let conversion = convert(&profile);
            let saved = save(store, backups, &conversion.tree, slot.map(index), now)?;
            let message = format!(
                "saved \"{}\" to slot {} ({} files, {} bytes)",
                profile.name, saved.slot, saved.files, saved.bytes
            );
            let mut notes = conversion.report.lines();
            notes.extend(leftover_notes(&saved.leftovers));
            Ok(done(message, Some(&saved.backup), notes))
        }
        OnboardRequest::Delete { slot } => {
            let name = slot_name(store, index(*slot))?;
            let changed = delete(store, backups, index(*slot), now)?;
            Ok(changed_reply(format!("deleted slot {slot} (\"{name}\")"), &changed))
        }
        OnboardRequest::Activate { slot } => {
            let name = slot_name(store, index(*slot))?;
            activate(store, index(*slot))?;
            Ok(done(format!("slot {slot} (\"{name}\") is active"), None, Vec::new()))
        }
        OnboardRequest::Backup { to } => {
            let snapshot = snapshot(store)?;
            let folder = match to {
                Some(to) => {
                    snapshot.write_to(Path::new(to))?;
                    Path::new(to).to_path_buf()
                }
                None => backups.save(&snapshot, now)?,
            };
            let message = format!("backed up {}", slots(slot_count(&snapshot)?));
            Ok(done(message, Some(&folder), Vec::new()))
        }
        OnboardRequest::Restore { from } => {
            let restored = slots(slot_count(&Snapshot::read_from(Path::new(from))?)?);
            let changed = restore(store, backups, Path::new(from), now)?;
            Ok(changed_reply(format!("restored {restored} from {from}"), &changed))
        }
    }
}

fn index(slot: u32) -> usize {
    usize::try_from(slot).unwrap_or(usize::MAX)
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn list_slots<S: FileStore + ?Sized>(store: &mut S) -> Result<Reply, OnboardError> {
    let listed = list(store)?;
    let slots = listed.slots.iter().map(wire_slot).collect();
    Ok(Reply::OnboardSlots { slots, active: listed.active, free_bytes: listed.free_bytes })
}

fn wire_slot(summary: &SlotSummary) -> OnboardSlot {
    OnboardSlot {
        index: count(summary.index),
        name: summary.name.clone(),
        files: count(summary.files),
        bytes: count(summary.bytes),
    }
}

fn slot_name<S: FileStore + ?Sized>(store: &mut S, slot: usize) -> Result<String, OnboardError> {
    let list = decode_list(&store.read(FileId::PROFILES_LIST)?)?;
    let root = *list.roots.get(slot).ok_or(OnboardError::NoSlot(slot))?;
    let root_bytes = store.read(FileId(root))?;
    Ok(ProfileRoot::decode(&root_bytes).map_err(|error| OnboardError::Decode(root, error))?.name)
}

fn slots(count: usize) -> String {
    if count == 1 { "1 slot".to_owned() } else { format!("{count} slots") }
}

fn slot_count(snapshot: &Snapshot) -> Result<usize, OnboardError> {
    Ok(decode_list(&snapshot.profiles_list)?.roots.len())
}

fn decode_list(bytes: &[u8]) -> Result<ProfilesList, OnboardError> {
    ProfilesList::decode(bytes).map_err(|error| OnboardError::Decode(FileId::PROFILES_LIST.0, error))
}

fn changed_reply(message: String, changed: &Changed) -> Reply {
    let mut notes: Vec<String> =
        changed.warnings.iter().map(|warning| format!("check the keyboard: {warning}")).collect();
    notes.extend(leftover_notes(&changed.leftovers));
    done(message, Some(&changed.backup), notes)
}

fn leftover_notes(leftovers: &[u16]) -> impl Iterator<Item = String> + '_ {
    leftovers.iter().map(|id| format!("file {id:#06x} was left on the keyboard"))
}

fn done(message: String, backup: Option<&Path>, notes: Vec<String>) -> Reply {
    Reply::OnboardDone { message, backup: backup.map(|path| path.display().to_string()), notes }
}

#[cfg(test)]
mod tests {
    use bragi::PropertyId;
    use engine::{EffectConfig, LayerConfig, NamedZone, Zone};
    use ipc::{OnboardRequest, OnboardSlot, Reply};
    use k100::Rgb;

    use super::*;
    use crate::fixtures::{now, red_profile, rig};
    use crate::{Failure, MemoryStore, OnboardError};

    fn red(name: &str) -> Option<ipc::Profile> {
        (name == "red").then(red_profile)
    }

    fn done(reply: Reply) -> (String, Option<String>, Vec<String>) {
        let Reply::OnboardDone { message, backup, notes } = reply else { panic!("expected done, got {reply:?}") };
        (message, backup, notes)
    }

    fn save_red(store: &mut MemoryStore, backups: &Backups) {
        let request = OnboardRequest::Save { profile: "red".into(), slot: None };
        execute(store, &request, &red, backups, now()).unwrap();
    }

    #[test]
    fn list_answers_with_the_slots_and_the_active_index() {
        let (mut store, backups, _temp) = rig();
        let reply = execute(&mut store, &OnboardRequest::List, &|_| None, &backups, now()).unwrap();
        let slot = OnboardSlot { index: 0, name: "HW Profile 2".into(), files: 16, bytes: 570 };
        assert_eq!(reply, Reply::OnboardSlots { slots: vec![slot], active: Some(0), free_bytes: None });
    }

    #[test]
    fn save_reports_the_slot_the_backup_and_what_was_left_out() {
        let (mut store, backups, _temp) = rig();
        let mut profile = red_profile();
        profile.layers.push(LayerConfig::new(
            "fade",
            Zone::Named(NamedZone::Logo),
            EffectConfig::Gradient { from: Rgb::BLACK, to: Rgb::BLACK, angle: 0.0 },
        ));
        let request = OnboardRequest::Save { profile: "red".into(), slot: None };
        let (message, backup, notes) = done(
            execute(&mut store, &request, &|name| (name == "red").then(|| profile.clone()), &backups, now()).unwrap(),
        );
        assert!(message.starts_with("saved \"red\" to slot 1 (6 files"), "{message}");
        assert!(backup.is_some());
        assert!(notes.iter().any(|note| note.contains("\"fade\"")), "{notes:?}");
    }

    #[test]
    fn an_unknown_profile_fails_before_the_keyboard_is_touched() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        let request = OnboardRequest::Save { profile: "nope".into(), slot: None };
        assert!(matches!(execute(&mut store, &request, &|_| None, &backups, now()), Err(OnboardError::NoProfile(_))));
        assert_eq!(store.files(), before);
        assert!(!backups.root().exists());
    }

    #[test]
    fn overwriting_a_slot_notes_every_old_file_it_could_not_remove() {
        let (mut store, backups, _temp) = rig();
        store.fail_remove(0x6D61, Failure::Timeout);
        let request = OnboardRequest::Save { profile: "red".into(), slot: Some(0) };
        let (message, _, notes) = done(execute(&mut store, &request, &red, &backups, now()).unwrap());
        assert!(message.starts_with("saved \"red\" to slot 0 ("), "{message}");
        assert!(notes.iter().any(|note| note.contains("0x6d61")), "{notes:?}");
    }

    #[test]
    fn delete_names_the_slot_it_removed_and_notes_warnings_and_leftovers() {
        let (mut store, backups, _temp) = rig();
        save_red(&mut store, &backups);
        store.put_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 1);
        store.fail_set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, Failure::Timeout);
        store.fail_remove(0x6D61, Failure::Timeout);
        let (message, backup, notes) =
            done(execute(&mut store, &OnboardRequest::Delete { slot: 0 }, &|_| None, &backups, now()).unwrap());
        assert_eq!(message, "deleted slot 0 (\"HW Profile 2\")");
        assert!(backup.is_some());
        assert!(
            notes.iter().any(|note| note.contains("check the keyboard") && note.contains("active slot")),
            "{notes:?}"
        );
        assert!(notes.iter().any(|note| note.contains("0x6d61")), "{notes:?}");
    }

    #[test]
    fn deleting_a_missing_slot_is_refused() {
        let (mut store, backups, _temp) = rig();
        let result = execute(&mut store, &OnboardRequest::Delete { slot: 3 }, &|_| None, &backups, now());
        assert!(matches!(result, Err(OnboardError::NoSlot(3))));
    }

    #[test]
    fn activate_names_the_slot_and_takes_no_backup() {
        let (mut store, backups, _temp) = rig();
        save_red(&mut store, &backups);
        let backups_before = std::fs::read_dir(backups.root()).unwrap().count();
        let (message, backup, notes) =
            done(execute(&mut store, &OnboardRequest::Activate { slot: 1 }, &|_| None, &backups, now()).unwrap());
        assert_eq!(message, "slot 1 (\"red\") is active");
        assert_eq!((backup, notes), (None, vec![]));
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(1));
        assert_eq!(std::fs::read_dir(backups.root()).unwrap().count(), backups_before);
    }

    #[test]
    fn backup_goes_to_the_chosen_folder_or_a_new_one() {
        let (mut store, backups, temp) = rig();
        let to = temp.path().join("chosen");
        let request = OnboardRequest::Backup { to: Some(to.display().to_string()) };
        let (message, backup, _) = done(execute(&mut store, &request, &|_| None, &backups, now()).unwrap());
        assert_eq!(message, "backed up 1 slot");
        assert_eq!(backup, Some(to.display().to_string()));
        assert!(to.join("6d70.bin").exists());
        let (_, backup, _) =
            done(execute(&mut store, &OnboardRequest::Backup { to: None }, &|_| None, &backups, now()).unwrap());
        assert!(std::path::Path::new(&backup.unwrap()).starts_with(backups.root()));
        save_red(&mut store, &backups);
        let (message, _, _) =
            done(execute(&mut store, &OnboardRequest::Backup { to: None }, &|_| None, &backups, now()).unwrap());
        assert_eq!(message, "backed up 2 slots");
    }

    #[test]
    fn restore_names_the_folder_and_the_backup_taken_before_it() {
        let (mut store, backups, temp) = rig();
        let original = store.files();
        let folder = temp.path().join("original").display().to_string();
        execute(&mut store, &OnboardRequest::Backup { to: Some(folder.clone()) }, &|_| None, &backups, now()).unwrap();
        save_red(&mut store, &backups);
        let request = OnboardRequest::Restore { from: folder.clone() };
        let (message, backup, notes) = done(execute(&mut store, &request, &|_| None, &backups, now()).unwrap());
        assert_eq!(message, format!("restored 1 slot from {folder}"));
        assert!(std::path::Path::new(&backup.unwrap()).starts_with(backups.root()));
        assert_eq!(notes, Vec::<String>::new());
        assert_eq!(store.files(), original);
    }
}
