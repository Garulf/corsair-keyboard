use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bragi::{FileId, OperatingMode, PropertyId, Status};
use k100::{Device, DeviceError, Transport};

use crate::{Backups, FREE_USE_IDS, OnboardError, OnboardTree, ProfileRoot, ProfilesList, Snapshot, decode_file};

pub const STORE_HANDLE: u8 = 3;

const PROFILES_LIST: FileId = FileId::PROFILES_LIST;

pub trait FileStore {
    fn read(&mut self, file: FileId) -> Result<Vec<u8>, DeviceError>;
    fn write(&mut self, file: FileId, data: &[u8]) -> Result<(), DeviceError>;
    fn create(&mut self, file: FileId) -> Result<(), DeviceError>;
    fn remove(&mut self, file: FileId) -> Result<(), DeviceError>;
    fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError>;
    fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError>;
}

impl<T: Transport> FileStore for Device<T> {
    fn read(&mut self, file: FileId) -> Result<Vec<u8>, DeviceError> {
        self.read_file(STORE_HANDLE, file)
    }

    fn write(&mut self, file: FileId, data: &[u8]) -> Result<(), DeviceError> {
        self.replace_file(STORE_HANDLE, file, data)
    }

    fn create(&mut self, file: FileId) -> Result<(), DeviceError> {
        self.create_file(file)
    }

    fn remove(&mut self, file: FileId) -> Result<(), DeviceError> {
        self.remove_file(file)
    }

    fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError> {
        Device::get_property(self, property)
    }

    fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
        Device::set_property(self, property, value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTree {
    pub root: u16,
    pub name: String,
    pub files: BTreeMap<u16, Vec<u8>>,
}

impl StoredTree {
    fn summary(&self, index: usize) -> SlotSummary {
        SlotSummary {
            index,
            root: self.root,
            name: self.name.clone(),
            files: self.files.len(),
            bytes: self.files.values().map(Vec::len).sum(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSummary {
    pub index: usize,
    pub root: u16,
    pub name: String,
    pub files: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotList {
    pub slots: Vec<SlotSummary>,
    pub active: Option<u32>,
    pub free_bytes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub slot: usize,
    pub root: u16,
    pub files: usize,
    pub bytes: usize,
    pub backup: PathBuf,
    pub leftovers: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub backup: PathBuf,
    pub leftovers: Vec<u16>,
    pub warnings: Vec<String>,
}

struct Slots {
    list_bytes: Vec<u8>,
    trees: Vec<StoredTree>,
    active: Option<u32>,
}

impl Slots {
    fn read<S: FileStore + ?Sized>(store: &mut S) -> Result<Self, OnboardError> {
        let list_bytes = store.read(PROFILES_LIST)?;
        let list = ProfilesList::decode(&list_bytes).map_err(|error| OnboardError::Decode(PROFILES_LIST.0, error))?;
        let trees = list.roots.iter().map(|root| read_tree(store, *root)).collect::<Result<_, _>>()?;
        let active = optional(store.get_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX))?;
        Ok(Self { list_bytes, trees, active })
    }

    fn roots(&self) -> Vec<u16> {
        self.trees.iter().map(|tree| tree.root).collect()
    }

    fn check(&self, slot: usize) -> Result<(), OnboardError> {
        if slot < self.trees.len() { Ok(()) } else { Err(OnboardError::NoSlot(slot)) }
    }

    fn used(&self) -> BTreeSet<u16> {
        self.trees.iter().flat_map(|tree| tree.files.keys().copied()).collect()
    }

    fn used_by_other_slots(&self, slot: usize) -> BTreeSet<u16> {
        let others = self.trees.iter().enumerate().filter(|(index, _)| *index != slot);
        others.flat_map(|(_, tree)| tree.files.keys().copied()).collect()
    }

    fn files_only_in(&self, slot: usize) -> Vec<u16> {
        let shared = self.used_by_other_slots(slot);
        self.trees[slot].files.keys().copied().filter(|id| !shared.contains(id)).collect()
    }

    fn snapshot(&self) -> Snapshot {
        let files = self.trees.iter().flat_map(|tree| tree.files.clone()).collect();
        Snapshot { profiles_list: self.list_bytes.clone(), files, active: self.active }
    }
}

fn optional(result: Result<u32, DeviceError>) -> Result<Option<u32>, DeviceError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.status().is_some() => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn read_tree<S: FileStore + ?Sized>(store: &mut S, root: u16) -> Result<StoredTree, OnboardError> {
    let root_bytes = store.read(FileId(root))?;
    let name = ProfileRoot::decode(&root_bytes).map_err(|error| OnboardError::Decode(root, error))?.name;
    let mut files = BTreeMap::new();
    let mut pending = VecDeque::from([(root, root_bytes)]);
    while let Some((id, bytes)) = pending.pop_front() {
        for child in children(&bytes) {
            if child != root && !files.contains_key(&child) && !pending.iter().any(|(queued, _)| *queued == child) {
                pending.push_back((child, store.read(FileId(child))?));
            }
        }
        files.insert(id, bytes);
    }
    Ok(StoredTree { root, name, files })
}

fn children(bytes: &[u8]) -> Vec<u16> {
    decode_file(bytes).map(|file| file.children()).unwrap_or_default()
}

pub fn snapshot<S: FileStore + ?Sized>(store: &mut S) -> Result<Snapshot, OnboardError> {
    Ok(Slots::read(store)?.snapshot())
}

pub fn list<S: FileStore + ?Sized>(store: &mut S) -> Result<SlotList, OnboardError> {
    let slots = Slots::read(store)?;
    let free_bytes = optional(store.get_property(PropertyId::FREE_STORAGE_SIZE))?;
    let summaries = slots.trees.iter().enumerate().map(|(index, tree)| tree.summary(index)).collect();
    Ok(SlotList { slots: summaries, active: slots.active, free_bytes })
}

pub fn save<S: FileStore + ?Sized>(
    store: &mut S,
    backups: &Backups,
    tree: &OnboardTree,
    slot: Option<usize>,
    now: SystemTime,
) -> Result<Saved, OnboardError> {
    let slots = Slots::read(store)?;
    if let Some(slot) = slot {
        slots.check(slot)?;
    }
    let backup = backups.save(&slots.snapshot(), now)?;

    let mut created = Vec::new();
    let files = match write_new_tree(store, tree, &slots.used(), &mut created) {
        Ok(files) => files,
        Err(error) => return Err(error.with_leftovers(remove_free_use(store, created))),
    };
    let root = files.last().expect("a tree always has a root").0;

    let mut roots = slots.roots();
    match slot {
        Some(slot) => roots[slot] = root,
        None => roots.push(root),
    }
    let slot_count = roots.len();
    replace_profiles_list(store, Some(&slots.list_bytes), &ProfilesList { roots }.encode())
        .map_err(|failure| failure.undo_with(|| remove_free_use(store, created)))?;

    let leftovers = slot.map(|replaced| remove_free_use(store, slots.files_only_in(replaced))).unwrap_or_default();
    Ok(Saved {
        slot: slot.unwrap_or(slot_count - 1),
        root,
        files: files.len(),
        bytes: files.iter().map(|(_, bytes)| bytes.len()).sum(),
        backup,
        leftovers,
    })
}

fn write_new_tree<S: FileStore + ?Sized>(
    store: &mut S,
    tree: &OnboardTree,
    used: &BTreeSet<u16>,
    created: &mut Vec<u16>,
) -> Result<Vec<(u16, Vec<u8>)>, OnboardError> {
    let mut candidates = FREE_USE_IDS.filter(|id| !used.contains(id));
    while created.len() < tree.file_count() {
        let id = candidates.next().ok_or(OnboardError::NoFreeIds)?;
        if read_if_present(store, id)?.is_some() {
            continue;
        }
        match store.create(FileId(id)) {
            Ok(()) => created.push(id),
            Err(refused) if refused.status().is_some() => {}
            Err(error) => return Err(error.into()),
        }
    }
    let files = tree.encode(created);
    for (id, bytes) in &files {
        write_verified(store, *id, bytes)?;
    }
    Ok(files)
}

fn write_verified<S: FileStore + ?Sized>(store: &mut S, id: u16, bytes: &[u8]) -> Result<(), OnboardError> {
    store.write(FileId(id), bytes)?;
    if store.read(FileId(id))? == bytes { Ok(()) } else { Err(OnboardError::Verify(id)) }
}

struct ListNotWritten {
    error: OnboardError,
    previous_list_back: bool,
}

impl ListNotWritten {
    fn undo_with(self, undo: impl FnOnce() -> Vec<u16>) -> OnboardError {
        if self.previous_list_back { self.error.with_leftovers(undo()) } else { self.error }
    }
}

fn replace_profiles_list<S: FileStore + ?Sized>(
    store: &mut S,
    previous: Option<&[u8]>,
    replacement: &[u8],
) -> Result<(), ListNotWritten> {
    let error = match write_verified(store, PROFILES_LIST.0, replacement) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let previous_list_back = previous.is_some_and(|previous| write_verified(store, PROFILES_LIST.0, previous).is_ok());
    Err(ListNotWritten { error, previous_list_back })
}

fn remove_free_use<S: FileStore + ?Sized>(store: &mut S, ids: impl IntoIterator<Item = u16>) -> Vec<u16> {
    ids.into_iter().filter(|id| FREE_USE_IDS.contains(id)).filter(|id| store.remove(FileId(*id)).is_err()).collect()
}

pub fn delete<S: FileStore + ?Sized>(
    store: &mut S,
    backups: &Backups,
    slot: usize,
    now: SystemTime,
) -> Result<Changed, OnboardError> {
    let slots = Slots::read(store)?;
    slots.check(slot)?;
    if slots.trees.len() == 1 {
        return Err(OnboardError::LastSlot);
    }
    let backup = backups.save(&slots.snapshot(), now)?;
    let mut roots = slots.roots();
    roots.remove(slot);
    replace_profiles_list(store, Some(&slots.list_bytes), &ProfilesList { roots }.encode())
        .map_err(|failure| failure.error)?;
    let mut warnings = Vec::new();
    if let Some(active) = slots.active {
        let shifted = match (active as usize).cmp(&slot) {
            std::cmp::Ordering::Less => active,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => active - 1,
        };
        if shifted != active {
            warnings.extend(set_active(store, shifted));
        }
    }
    let leftovers = remove_free_use(store, slots.files_only_in(slot));
    Ok(Changed { backup, leftovers, warnings })
}

fn set_active<S: FileStore + ?Sized>(store: &mut S, index: u32) -> Option<String> {
    let result = store.set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, index);
    result.err().map(|error| format!("the active slot could not be set to {index}: {error}"))
}

pub fn activate<S: FileStore + ?Sized>(store: &mut S, slot: usize) -> Result<(), OnboardError> {
    let list = ProfilesList::decode(&store.read(PROFILES_LIST)?)
        .map_err(|error| OnboardError::Decode(PROFILES_LIST.0, error))?;
    if slot >= list.roots.len() {
        return Err(OnboardError::NoSlot(slot));
    }
    let index = u32::try_from(slot).map_err(|_| OnboardError::NoSlot(slot))?;
    store.set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, index)?;
    Ok(())
}

pub fn restore<S: FileStore + ?Sized>(
    store: &mut S,
    backups: &Backups,
    from: &Path,
    now: SystemTime,
) -> Result<Changed, OnboardError> {
    let restored = Snapshot::read_from(from)?;
    let restored_roots = validated_roots(&restored, from)?;
    let current = LiveState::read(store)?;
    let backup = backups.save(&current.snapshot(), now)?;

    let mut undo = Undo::default();
    if let Err(error) = write_snapshot_files(store, &restored, &current.files, &mut undo) {
        return Err(error.with_leftovers(undo.apply(store)));
    }
    replace_profiles_list(store, current.list_bytes.as_deref(), &restored.profiles_list)
        .map_err(|failure| failure.undo_with(|| undo.apply(store)))?;
    let mut warnings = Vec::new();
    if let Some(active) = restored.active {
        let index = if (active as usize) < restored_roots.len() { active } else { 0 };
        warnings.extend(set_active(store, index));
    }
    let not_restored = |id: &u16| !restored.files.contains_key(id);
    let newer: Vec<u16> = current.attributed.iter().copied().filter(not_restored).collect();
    let mut leftovers = remove_free_use(store, newer);
    leftovers.extend(current.files.keys().copied().filter(|id| !current.attributed.contains(id) && not_restored(id)));
    leftovers.sort_unstable();
    Ok(Changed { backup, leftovers, warnings })
}

struct LiveState {
    list_bytes: Option<Vec<u8>>,
    files: BTreeMap<u16, Vec<u8>>,
    attributed: BTreeSet<u16>,
    active: Option<u32>,
}

impl LiveState {
    fn read<S: FileStore + ?Sized>(store: &mut S) -> Result<Self, OnboardError> {
        let list_bytes = read_if_present(store, PROFILES_LIST.0)?;
        let roots = list_bytes.as_deref().and_then(|bytes| ProfilesList::decode(bytes).ok()).map(|list| list.roots);
        let mut state = Self {
            list_bytes,
            files: BTreeMap::new(),
            attributed: BTreeSet::new(),
            active: optional(store.get_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX))?,
        };
        for root in roots.unwrap_or_default() {
            let walk = walk_tree(store, root)?;
            if walk.complete {
                state.attributed.extend(walk.files.keys().copied());
            }
            state.files.extend(walk.files);
        }
        Ok(state)
    }

    fn snapshot(&self) -> Snapshot {
        let files = self.files.iter().filter(|(id, _)| FREE_USE_IDS.contains(id));
        Snapshot {
            profiles_list: self.list_bytes.clone().unwrap_or_default(),
            files: files.map(|(id, bytes)| (*id, bytes.clone())).collect(),
            active: self.active,
        }
    }
}

struct Walk {
    files: BTreeMap<u16, Vec<u8>>,
    complete: bool,
}

fn walk_tree<S: FileStore + ?Sized>(store: &mut S, root: u16) -> Result<Walk, OnboardError> {
    let mut walk = Walk { files: BTreeMap::new(), complete: true };
    let mut pending = VecDeque::from([root]);
    while let Some(id) = pending.pop_front() {
        if walk.files.contains_key(&id) {
            continue;
        }
        let Some(bytes) = read_if_present(store, id)? else {
            walk.complete = false;
            continue;
        };
        if id == root && ProfileRoot::decode(&bytes).is_err() {
            walk.complete = false;
        } else {
            pending.extend(children(&bytes));
        }
        walk.files.insert(id, bytes);
    }
    Ok(walk)
}

fn read_if_present<S: FileStore + ?Sized>(store: &mut S, id: u16) -> Result<Option<Vec<u8>>, OnboardError> {
    match store.read(FileId(id)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(missing) if missing.status() == Some(Status::InvalidDeviceAddress) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn validated_roots(snapshot: &Snapshot, dir: &Path) -> Result<Vec<u16>, OnboardError> {
    let not_a_backup = || OnboardError::NotABackup(dir.to_path_buf());
    let list = ProfilesList::decode(&snapshot.profiles_list).map_err(|_| not_a_backup())?;
    let mut pending: Vec<u16> = list.roots.clone();
    let mut seen = BTreeSet::new();
    for root in &list.roots {
        ProfileRoot::decode(snapshot.files.get(root).ok_or_else(not_a_backup)?).map_err(|_| not_a_backup())?;
    }
    while let Some(id) = pending.pop() {
        if seen.insert(id) {
            pending.extend(children(snapshot.files.get(&id).ok_or_else(not_a_backup)?));
        }
    }
    Ok(list.roots)
}

#[derive(Default)]
struct Undo {
    created: Vec<u16>,
    overwritten: Vec<(u16, Vec<u8>)>,
}

impl Undo {
    fn apply<S: FileStore + ?Sized>(&self, store: &mut S) -> Vec<u16> {
        let not_put_back =
            self.overwritten.iter().filter(|(id, previous)| write_verified(store, *id, previous).is_err());
        let mut leftovers: Vec<u16> = not_put_back.map(|(id, _)| *id).collect();
        leftovers.extend(remove_free_use(store, self.created.iter().copied()));
        leftovers.sort_unstable();
        leftovers
    }
}

fn write_snapshot_files<S: FileStore + ?Sized>(
    store: &mut S,
    snapshot: &Snapshot,
    known: &BTreeMap<u16, Vec<u8>>,
    undo: &mut Undo,
) -> Result<(), OnboardError> {
    for (id, bytes) in &snapshot.files {
        let current = match known.get(id) {
            Some(current) => Some(current.clone()),
            None => read_if_present(store, *id)?,
        };
        match current {
            Some(current) if current == *bytes => continue,
            Some(current) => undo.overwritten.push((*id, current)),
            None => match store.create(FileId(*id)) {
                Ok(()) => undo.created.push(*id),
                Err(refused) if refused.status().is_some() => {}
                Err(error) => return Err(error.into()),
            },
        }
        write_verified(store, *id, bytes)?;
    }
    Ok(())
}

pub fn replay_hardware_profile<T: Transport>(device: &mut Device<T>, key_count: usize) -> Result<(), DeviceError> {
    device.set_property(PropertyId::OPERATING_MODE, OperatingMode::Software.raw())?;
    k100::restore(device, key_count)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use bragi::Status;
    use k100::fake::{FakeTransport, ok_reply};

    use super::*;
    use crate::fixtures::{self, now, red_profile, rig};
    use crate::{FREE_USE_IDS, Failure, KEY_COUNT, Operation, ProfileRoot, ProfilesList, convert};

    fn red_tree() -> OnboardTree {
        convert(&red_profile()).tree
    }

    const RED_IDS: [u16; 6] = [0x6D6F, 0x6D71, 0x6D72, 0x6D73, 0x6D74, 0x6D75];
    const RED_FILE_WRITES: usize = 6;
    const ICUE_TREE: [u16; 16] = [
        0x6D60, 0x6D61, 0x6D62, 0x6D63, 0x6D64, 0x6D65, 0x6D66, 0x6D67, 0x6D68, 0x6D69, 0x6D6A, 0x6D6B, 0x6D6C, 0x6D6D,
        0x6D6E, 0x6D70,
    ];

    #[test]
    fn listing_the_dump_shows_the_icue_profile() {
        let (mut store, _, _temp) = rig();
        let list = list(&mut store).unwrap();
        assert_eq!((list.active, list.free_bytes), (Some(0), None));
        let summary = SlotSummary { index: 0, root: 0x6D70, name: "HW Profile 2".into(), files: 16, bytes: 570 };
        assert_eq!(list.slots, [summary]);
    }

    #[test]
    fn listing_reads_the_free_space_when_the_keyboard_reports_it() {
        let (mut store, _, _temp) = rig();
        store.put_property(PropertyId::FREE_STORAGE_SIZE, 8_000_000);
        assert_eq!(list(&mut store).unwrap().free_bytes, Some(8_000_000));
    }

    #[test]
    fn reading_a_tree_follows_every_section_including_the_archive() {
        let (mut store, _, _temp) = rig();
        let tree = read_tree(&mut store, 0x6D70).unwrap();
        let mut expected: Vec<u16> = (0x6D60..=0x6D6E).collect();
        expected.push(0x6D70);
        assert_eq!(tree.files.keys().copied().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn reading_a_tree_with_a_missing_file_is_an_error() {
        let store = fixtures::dump_store();
        let mut files = store.files();
        files.remove(&0x6D6B);
        let mut store = crate::MemoryStore::with_files(files);
        let error = read_tree(&mut store, 0x6D70).unwrap_err();
        assert!(
            matches!(&error, OnboardError::Device(device) if device.status() == Some(Status::InvalidDeviceAddress))
        );
    }

    #[test]
    fn saving_a_new_slot_uses_the_lowest_free_ids_and_appends_the_root() {
        let (mut store, backups, _temp) = rig();
        let saved = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        assert_eq!((saved.slot, saved.root, saved.files), (1, 0x6D75, 6));
        let files = store.files();
        assert_eq!(files[&0x0F], [0x13, 0x00, 0x02, 0x00, 0x70, 0x6D, 0x75, 0x6D]);
        for id in [0x6D6F, 0x6D71, 0x6D72, 0x6D73, 0x6D74, 0x6D75] {
            assert!(files.contains_key(&id), "{id:#x}");
        }
        assert_eq!(ProfileRoot::decode(&files[&0x6D75]).unwrap().name, "red");
    }

    #[test]
    fn a_saved_tree_reads_back_as_written() {
        let (mut store, backups, _temp) = rig();
        let saved = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        let written: BTreeMap<u16, Vec<u8>> = red_tree().encode(&RED_IDS).into_iter().collect();
        assert_eq!(read_tree(&mut store, saved.root).unwrap().files, written);
        assert_eq!(saved.bytes, written.values().map(Vec::len).sum::<usize>());
        assert!(saved.leftovers.is_empty());
    }

    #[test]
    fn an_id_the_keyboard_refuses_to_create_is_skipped() {
        let (mut store, backups, _temp) = rig();
        store.refuse_create(0x6D6F);
        assert_eq!(save(&mut store, &backups, &red_tree(), None, now()).unwrap().root, 0x6D76);
        assert!(!store.files().contains_key(&0x6D6F));
    }

    #[test]
    fn an_unreferenced_file_at_the_first_free_id_is_skipped_and_left_untouched() {
        let (mut store, backups, _temp) = rig();
        let stray = vec![0x6E, 0x07, 0x02, 0, 0, 0, 0, 0, 0x03];
        store.insert(0x6D6F, stray.clone());
        let saved = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        assert_eq!(saved.root, 0x6D76);
        assert_eq!(store.files()[&0x6D6F], stray);
        let touches_stray = |op: &Operation| matches!(op, Operation::Create(0x6D6F) | Operation::Write(0x6D6F));
        assert!(!store.operations().iter().any(touches_stray), "{:?}", store.operations());
    }

    #[test]
    fn a_probe_the_keyboard_does_not_answer_with_missing_aborts_the_save_and_removes_what_it_created() {
        for failure in [Failure::Status(Status::HardwareError), Failure::Timeout] {
            let (mut store, backups, _temp) = rig();
            let before = store.files();
            store.fail_read(0x6D72, failure);
            let error = save(&mut store, &backups, &red_tree(), None, now()).unwrap_err();
            assert_eq!(error.lost_device(), failure == Failure::Timeout, "{error}");
            assert_eq!(store.files(), before);
            assert!(!store.operations().contains(&Operation::Create(0x6D72)), "{:?}", store.operations());
        }
    }

    #[test]
    fn a_create_that_times_out_aborts_the_save_and_removes_what_it_created() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        store.fail_create(0x6D72, Failure::Timeout);
        let error = save(&mut store, &backups, &red_tree(), None, now()).unwrap_err();
        assert!(error.lost_device(), "{error}");
        assert_eq!(store.files(), before);
        let creates: Vec<Operation> =
            store.operations().into_iter().filter(|op| matches!(op, Operation::Create(_))).collect();
        assert_eq!(creates, [Operation::Create(0x6D6F), Operation::Create(0x6D71), Operation::Create(0x6D72)]);
    }

    #[test]
    fn running_out_of_free_ids_removes_what_it_created() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        for id in 0x6D72..=*FREE_USE_IDS.end() {
            store.refuse_create(id);
        }
        assert!(matches!(save(&mut store, &backups, &red_tree(), None, now()), Err(OnboardError::NoFreeIds)));
        assert_eq!(store.files(), before);
    }

    #[test]
    fn overwriting_writes_the_new_tree_then_swaps_the_root_then_removes_the_old_one() {
        let (mut store, backups, _temp) = rig();
        let saved = save(&mut store, &backups, &red_tree(), Some(0), now()).unwrap();
        assert_eq!((saved.slot, saved.root), (0, 0x6D75));
        let files = store.files();
        assert_eq!(files[&0x0F], [0x13, 0x00, 0x01, 0x00, 0x75, 0x6D]);
        let mut expected = vec![0x0F, 0x25, 0x6D6F];
        expected.extend(0x6D71..=0x6D75);
        assert_eq!(files.keys().copied().collect::<Vec<_>>(), expected);
        let operations = store.operations();
        let list_write = operations.iter().position(|op| *op == Operation::Write(0x0F)).unwrap();
        let last_create = operations.iter().rposition(|op| matches!(op, Operation::Create(_))).unwrap();
        let first_remove = operations.iter().position(|op| matches!(op, Operation::Remove(_))).unwrap();
        assert!(last_create < list_write && list_write < first_remove, "{operations:?}");
    }

    #[test]
    fn overwriting_keeps_files_another_slot_still_uses() {
        let (mut store, backups, _temp) = rig();
        store.insert(0x0F, ProfilesList { roots: vec![0x6D70, 0x6D70] }.encode());
        save(&mut store, &backups, &red_tree(), Some(0), now()).unwrap();
        let files = store.files();
        assert!(ICUE_TREE.iter().all(|id| files.contains_key(id)));
        assert_eq!(ProfilesList::decode(&files[&0x0F]).unwrap().roots, [0x6D75, 0x6D70]);
    }

    #[test]
    fn files_that_cannot_be_removed_are_reported_as_leftovers() {
        let (mut store, backups, _temp) = rig();
        store.fail_remove(0x6D61, Failure::Status(Status::HardwareError));
        let saved = save(&mut store, &backups, &red_tree(), Some(0), now()).unwrap();
        assert_eq!(saved.leftovers, [0x6D61]);
        let files = store.files();
        assert_eq!(ProfilesList::decode(&files[&0x0F]).unwrap().roots, [0x6D75]);
        assert!(files.contains_key(&0x6D61) && !files.contains_key(&0x6D60));
    }

    #[test]
    fn a_slot_past_the_end_is_refused_before_anything_is_written() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        assert!(matches!(save(&mut store, &backups, &red_tree(), Some(1), now()), Err(OnboardError::NoSlot(1))));
        assert_eq!(store.files(), before);
        assert!(!backups.root().exists());
    }

    #[test]
    fn a_failed_write_removes_what_it_created_and_leaves_the_list_alone() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        store.fail_writes_from(3, Failure::Status(Status::HardwareError));
        assert!(save(&mut store, &backups, &red_tree(), None, now()).is_err());
        assert_eq!(store.files(), before);
    }

    #[test]
    fn a_write_that_reads_back_different_is_undone() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        store.mangle_writes();
        assert!(matches!(save(&mut store, &backups, &red_tree(), None, now()), Err(OnboardError::Verify(_))));
        assert_eq!(store.files(), before);
    }

    #[test]
    fn a_profiles_list_that_reads_back_different_is_put_back_and_the_new_files_removed() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        store.mangle_write(RED_FILE_WRITES + 1);
        let result = save(&mut store, &backups, &red_tree(), Some(0), now());
        assert!(matches!(result, Err(OnboardError::Verify(0x0F))), "{result:?}");
        assert_eq!(store.files(), before);
    }

    #[test]
    fn a_failed_profiles_list_write_is_put_back_and_the_new_files_removed() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        store.fail_write(RED_FILE_WRITES + 1, Failure::Timeout);
        let error = save(&mut store, &backups, &red_tree(), None, now()).unwrap_err();
        assert!(error.lost_device());
        assert_eq!(store.files(), before);
    }

    #[test]
    fn when_the_old_list_cannot_be_put_back_the_new_files_stay() {
        let (mut store, backups, _temp) = rig();
        store.mangle_write(RED_FILE_WRITES + 1);
        store.fail_writes_from(RED_FILE_WRITES + 2, Failure::Timeout);
        assert!(save(&mut store, &backups, &red_tree(), None, now()).is_err());
        let files = store.files();
        assert!(ICUE_TREE.iter().chain(&RED_IDS).all(|id| files.contains_key(id)));
        assert!(!store.operations().iter().any(|op| matches!(op, Operation::Remove(_))));
    }

    #[test]
    fn every_change_is_backed_up_first() {
        let (mut store, backups, temp) = rig();
        let saved = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        assert_eq!(saved.backup, temp.path().join("onboard-backup").join("20261017T120101Z"));
        assert_eq!(std::fs::read(saved.backup.join("000f.bin")).unwrap(), fixtures::file(0x0F));
        assert_eq!(std::fs::read(saved.backup.join("6d70.bin")).unwrap(), fixtures::file(0x6D70));
        assert_eq!(std::fs::read_to_string(saved.backup.join("active.txt")).unwrap().trim(), "0");
        let second = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        assert_eq!(second.backup.file_name().unwrap(), "20261017T120101Z-2");
    }

    #[test]
    fn a_backup_holds_every_file_of_every_slot() {
        let (mut store, backups, _temp) = rig();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        let changed = delete(&mut store, &backups, 1, now()).unwrap();
        let backed_up = Snapshot::read_from(&changed.backup).unwrap();
        let mut expected: Vec<u16> = ICUE_TREE.into();
        expected.extend(RED_IDS);
        expected.sort();
        assert_eq!(backed_up.files.keys().copied().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn deleting_removes_the_slot_and_its_files_and_keeps_the_active_index_valid() {
        let (mut store, backups, _temp) = rig();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        store.put_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 1);
        delete(&mut store, &backups, 0, now()).unwrap();
        let files = store.files();
        assert_eq!(files[&0x0F], [0x13, 0x00, 0x01, 0x00, 0x75, 0x6D]);
        assert!(!(0x6D60..=0x6D6E).chain([0x6D70]).any(|id| files.contains_key(&id)));
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(0));
    }

    #[test]
    fn deleting_the_active_slot_activates_the_first_and_a_higher_slot_is_untouched() {
        let (mut store, backups, _temp) = rig();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        store.put_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 1);
        delete(&mut store, &backups, 1, now()).unwrap();
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(0));
        let operations_before = store.operations().len();
        delete(&mut store, &backups, 1, now()).unwrap();
        let set_after: Vec<Operation> = store.operations()[operations_before..]
            .iter()
            .copied()
            .filter(|op| matches!(op, Operation::SetProperty(..)))
            .collect();
        assert!(set_after.is_empty(), "{set_after:?}");
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(0));
    }

    #[test]
    fn deleting_keeps_files_another_slot_still_uses() {
        let (mut store, backups, _temp) = rig();
        store.insert(0x0F, ProfilesList { roots: vec![0x6D70, 0x6D70] }.encode());
        delete(&mut store, &backups, 0, now()).unwrap();
        let files = store.files();
        assert!(ICUE_TREE.iter().all(|id| files.contains_key(id)));
        assert_eq!(ProfilesList::decode(&files[&0x0F]).unwrap().roots, [0x6D70]);
    }

    #[test]
    fn a_failed_list_write_on_delete_removes_nothing() {
        let (mut store, backups, _temp) = rig();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        let before = store.files();
        store.mangle_write(RED_FILE_WRITES + 2);
        assert!(matches!(delete(&mut store, &backups, 0, now()), Err(OnboardError::Verify(0x0F))));
        assert_eq!(store.files(), before);
    }

    #[test]
    fn the_last_slot_cannot_be_deleted() {
        let (mut store, backups, _temp) = rig();
        let before = store.files();
        assert!(matches!(delete(&mut store, &backups, 0, now()), Err(OnboardError::LastSlot)));
        assert!(matches!(delete(&mut store, &backups, 3, now()), Err(OnboardError::NoSlot(3))));
        assert_eq!(store.files(), before);
    }

    #[test]
    fn activating_sets_the_hardware_profile_index_and_checks_the_slot() {
        let (mut store, backups, _temp) = rig();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        activate(&mut store, 1).unwrap();
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(1));
        assert!(matches!(activate(&mut store, 5), Err(OnboardError::NoSlot(5))));
    }

    #[test]
    fn restore_puts_back_the_backed_up_slots_and_removes_newer_files() {
        let (mut store, backups, _temp) = rig();
        let original = store.files();
        let saved = save(&mut store, &backups, &red_tree(), Some(0), now()).unwrap();
        restore(&mut store, &backups, &saved.backup, now()).unwrap();
        assert_eq!(store.files(), original);
    }

    #[test]
    fn restore_sets_the_backed_up_active_index_or_the_first_slot_when_it_is_past_the_end() {
        let (mut store, backups, temp) = rig();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        let mut snapshot = snapshot(&mut store).unwrap();
        snapshot.active = Some(1);
        let one = temp.path().join("one");
        snapshot.write_to(&one).unwrap();
        snapshot.active = Some(7);
        let seven = temp.path().join("seven");
        snapshot.write_to(&seven).unwrap();
        restore(&mut store, &backups, &one, now()).unwrap();
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(1));
        restore(&mut store, &backups, &seven, now()).unwrap();
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(0));
    }

    #[test]
    fn restore_refuses_a_folder_that_is_not_a_backup() {
        let (mut store, backups, temp) = rig();
        let before = store.files();
        let empty = temp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(restore(&mut store, &backups, &empty, now()).is_err());
        let foreign = temp.path().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("000f.bin"), fixtures::file(0x0F)).unwrap();
        std::fs::write(foreign.join("0002.bin"), [0x01]).unwrap();
        assert!(matches!(restore(&mut store, &backups, &foreign, now()), Err(OnboardError::NotFreeUse(2))));
        assert_eq!(store.files(), before);
    }

    #[test]
    fn restore_refuses_a_backup_whose_list_points_at_missing_files() {
        let (mut store, backups, temp) = rig();
        let before = store.files();
        let mut incomplete = snapshot(&mut store).unwrap();
        incomplete.files.remove(&0x6D6B);
        let dir = temp.path().join("incomplete");
        incomplete.write_to(&dir).unwrap();
        assert!(matches!(restore(&mut store, &backups, &dir, now()), Err(OnboardError::NotABackup(_))));
        assert_eq!(store.files(), before);
        assert!(store.operations().is_empty());
    }

    #[test]
    fn a_failed_restore_puts_the_previous_files_back() {
        let (mut store, backups, _temp) = rig();
        let saved = save(&mut store, &backups, &red_tree(), Some(0), now()).unwrap();
        let before = store.files();
        let writes_so_far = store.operations().iter().filter(|op| matches!(op, Operation::Write(_))).count();
        store.fail_write(writes_so_far + 4, Failure::Status(Status::HardwareError));
        assert!(restore(&mut store, &backups, &saved.backup, now()).is_err());
        assert_eq!(store.files(), before);
    }

    #[test]
    fn a_failed_restore_writes_back_the_files_it_had_overwritten() {
        let (mut store, backups, temp) = rig();
        let before = store.files();
        let mut changed = snapshot(&mut store).unwrap();
        let red_files = red_tree().encode(&RED_IDS);
        changed.files.insert(0x6D6B, red_files[1].1.clone());
        changed.files.insert(0x6D6C, red_files[2].1.clone());
        let dir = temp.path().join("changed");
        changed.write_to(&dir).unwrap();
        store.fail_write(2, Failure::Status(Status::HardwareError));
        assert!(matches!(restore(&mut store, &backups, &dir, now()), Err(OnboardError::Device(_))));
        assert_eq!(store.files(), before);
    }

    #[test]
    fn restore_stops_when_a_file_read_fails_for_a_reason_other_than_missing() {
        let (mut store, backups, temp) = rig();
        store.insert(0x6D80, vec![0x6E, 0x07, 0x02, 0, 0, 0, 0, 0, 0x03]);
        let before = store.files();
        let mut with_extra = snapshot(&mut store).unwrap();
        with_extra.files.insert(0x6D80, vec![0x6E, 0x07, 0x02, 0, 0, 0, 0, 0, 0x04]);
        let dir = temp.path().join("extra");
        with_extra.write_to(&dir).unwrap();
        store.fail_read(0x6D80, Failure::Status(Status::HardwareError));
        let error = restore(&mut store, &backups, &dir, now()).unwrap_err();
        assert_eq!(error.device().and_then(DeviceError::status), Some(Status::HardwareError), "{error}");
        assert_eq!(store.files(), before);
    }

    #[test]
    fn restore_skips_files_that_already_hold_the_backed_up_bytes() {
        let (mut store, backups, _temp) = rig();
        let saved = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        let operations_before = store.operations().len();
        restore(&mut store, &backups, &saved.backup, now()).unwrap();
        let restore_operations: Vec<Operation> = store.operations()[operations_before..]
            .iter()
            .copied()
            .filter(|op| matches!(op, Operation::Create(_) | Operation::Write(_)))
            .collect();
        assert_eq!(restore_operations, [Operation::Write(0x0F)]);
        assert!(!RED_IDS.iter().any(|id| store.files().contains_key(id)));
    }

    #[test]
    fn restore_runs_when_a_live_root_is_unreadable_and_keeps_what_it_cannot_attribute() {
        let (mut store, backups, _temp) = rig();
        let original = store.files();
        let saved = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        let mut corrupt_root = store.files()[&0x6D75].clone();
        corrupt_root.truncate(10);
        store.insert(0x6D75, corrupt_root);
        let changed = restore(&mut store, &backups, &saved.backup, now()).unwrap();
        assert_eq!(changed.leftovers, [0x6D75]);
        let files = store.files();
        assert_eq!(files[&0x0F], original[&0x0F]);
        assert!(ICUE_TREE.iter().all(|id| files[id] == original[id]));
        assert!(RED_IDS.iter().all(|id| files.contains_key(id)));
        assert!(changed.backup.join("6d75.bin").exists());
    }

    #[test]
    fn restore_recovers_after_an_interrupted_restore_left_a_slot_root_holding_backup_bytes() {
        let (mut store, backups, _temp) = rig();
        let original = store.files();
        let first = save(&mut store, &backups, &red_tree(), Some(0), now()).unwrap();
        let second = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        assert_eq!(second.root, 0x6D65);
        let writes_so_far = store.operations().iter().filter(|op| matches!(op, Operation::Write(_))).count();
        store.fail_writes_from(writes_so_far + 7, Failure::Timeout);
        let interrupted = restore(&mut store, &backups, &first.backup, now()).unwrap_err();
        assert!(interrupted.lost_device());
        assert_eq!(interrupted.leftovers(), (0x6D60..=0x6D65).collect::<Vec<u16>>());
        assert!(read_tree(&mut store, 0x6D65).is_err());
        store.stop_failing();
        restore(&mut store, &backups, &first.backup, now()).unwrap();
        assert_eq!(store.files(), original);
    }

    #[test]
    fn a_rollback_that_cannot_remove_a_created_file_reports_it() {
        let (mut store, backups, _temp) = rig();
        store.fail_writes_from(3, Failure::Status(Status::HardwareError));
        store.fail_remove(0x6D6F, Failure::Timeout);
        let error = save(&mut store, &backups, &red_tree(), None, now()).unwrap_err();
        assert_eq!(error.leftovers(), [0x6D6F]);
        assert!(
            matches!(&error, OnboardError::PartlyUndone { error, .. } if matches!(**error, OnboardError::Device(_)))
        );
        assert!(!error.lost_device());
        assert_eq!(ProfilesList::decode(&store.files()[&0x0F]).unwrap().roots, [0x6D70]);
    }

    #[test]
    fn a_delete_whose_active_index_write_fails_still_reports_the_backup() {
        let (mut store, backups, _temp) = rig();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        store.put_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 1);
        store.fail_set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, Failure::Status(Status::HardwareError));
        let changed = delete(&mut store, &backups, 0, now()).unwrap();
        assert!(changed.backup.join("000f.bin").exists());
        assert_eq!(changed.warnings.len(), 1, "{:?}", changed.warnings);
        let files = store.files();
        assert_eq!(ProfilesList::decode(&files[&0x0F]).unwrap().roots, [0x6D75]);
        assert!(!files.contains_key(&0x6D70));
    }

    #[test]
    fn a_restore_whose_active_index_write_fails_still_reports_the_backup() {
        let (mut store, backups, _temp) = rig();
        let original = store.files();
        let saved = save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        store.fail_set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, Failure::Timeout);
        let changed = restore(&mut store, &backups, &saved.backup, now()).unwrap();
        assert!(changed.backup.join("000f.bin").exists());
        assert_eq!(changed.warnings.len(), 1, "{:?}", changed.warnings);
        assert_eq!(store.files(), original);
    }

    #[test]
    fn nothing_outside_the_free_use_ids_but_the_profiles_list_is_written_or_removed() {
        let (mut store, backups, _temp) = rig();
        let first = save(&mut store, &backups, &red_tree(), Some(0), now()).unwrap();
        save(&mut store, &backups, &red_tree(), None, now()).unwrap();
        delete(&mut store, &backups, 0, now()).unwrap();
        restore(&mut store, &backups, &first.backup, now()).unwrap();
        for operation in store.operations() {
            let id = match operation {
                Operation::Create(id) | Operation::Remove(id) => id,
                Operation::Write(0x0F) => continue,
                Operation::Write(id) => id,
                Operation::SetProperty(property, _) => {
                    assert_eq!(property, PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX.0);
                    continue;
                }
            };
            assert!(FREE_USE_IDS.contains(&id), "{operation:?}");
        }
    }

    fn one_file_keyboard() -> FakeTransport {
        let contents = Arc::new(Mutex::new(Vec::new()));
        FakeTransport::with_responder(move |request| {
            let mut reply = ok_reply(request);
            let mut contents = contents.lock().unwrap();
            match request[1] {
                0x06 => {
                    let len = u32::from_le_bytes(request[3..7].try_into().unwrap()) as usize;
                    *contents = request[7..7 + len].to_vec();
                }
                0x09 => {
                    reply.extend_from_slice(&[0, 0]);
                    reply.extend_from_slice(&(contents.len() as u32).to_le_bytes());
                }
                0x08 => reply.extend_from_slice(&contents),
                _ => {}
            }
            vec![reply]
        })
    }

    #[test]
    fn the_keyboard_store_reads_and_writes_on_handle_three() {
        let fake = one_file_keyboard();
        let log = fake.log();
        let mut device = Device::new(fake);
        FileStore::write(&mut device, FileId(0x6D71), &[1, 2]).unwrap();
        assert_eq!(&log.lock().unwrap()[0][..5], [0x08, 0x0D, STORE_HANDLE, 0x71, 0x6D]);
        log.lock().unwrap().clear();
        assert_eq!(FileStore::read(&mut device, FileId(0x6D71)).unwrap(), [1, 2]);
        let log = log.lock().unwrap();
        assert_eq!(&log[0][..5], [0x08, 0x0D, STORE_HANDLE, 0x71, 0x6D]);
        assert!(log.iter().filter(|report| matches!(report[1], 0x08 | 0x09)).all(|report| report[2] == STORE_HANDLE));
    }

    #[test]
    fn the_keyboard_store_creates_removes_and_sets_properties() {
        let fake = FakeTransport::new();
        let log = fake.log();
        let mut device = Device::new(fake);
        FileStore::create(&mut device, FileId(0x6D71)).unwrap();
        FileStore::remove(&mut device, FileId(0x6D71)).unwrap();
        FileStore::set_property(&mut device, PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 1).unwrap();
        let log = log.lock().unwrap();
        assert_eq!(&log[0][..4], [0x08, 0x0B, 0x71, 0x6D]);
        assert_eq!(&log[1][..4], [0x08, 0x0C, 0x71, 0x6D]);
        assert_eq!(&log[2][..8], [0x08, 0x01, 0x6E, 0x00, 1, 0, 0, 0]);
    }

    #[test]
    fn replaying_the_hardware_profile_cycles_the_operating_mode() {
        let fake = FakeTransport::new();
        let log = fake.log();
        replay_hardware_profile(&mut Device::new(fake), KEY_COUNT).unwrap();
        let log = log.lock().unwrap();
        assert_eq!(&log[0][..8], [0x08, 0x01, 0x03, 0x00, 2, 0, 0, 0]);
        assert_eq!(&log.last().unwrap()[..8], [0x08, 0x01, 0x03, 0x00, 1, 0, 0, 0]);
    }
}
