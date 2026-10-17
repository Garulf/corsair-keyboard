use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use bragi::{Command, FileId, PropertyId, Status};
use k100::DeviceError;

use crate::FileStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Status(Status),
    Timeout,
}

impl Failure {
    fn error(self, command: Command) -> DeviceError {
        match self {
            Self::Status(status) => DeviceError::Status { command, status },
            Self::Timeout => DeviceError::Timeout(command),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Create(u16),
    Write(u16),
    Remove(u16),
    SetProperty(u16, u32),
}

#[derive(Default)]
struct State {
    files: BTreeMap<u16, Vec<u8>>,
    properties: BTreeMap<u16, u32>,
    create_failures: BTreeMap<u16, Failure>,
    remove_failures: BTreeMap<u16, Failure>,
    set_property_failures: BTreeMap<u16, Failure>,
    writes_failing_from: Option<(usize, Failure)>,
    single_write_failures: BTreeMap<usize, Failure>,
    mangle_every_write: bool,
    single_mangled_writes: BTreeSet<usize>,
    writes: usize,
    operations: Vec<Operation>,
}

impl State {
    fn write_failure(&self) -> Option<Failure> {
        let from = self.writes_failing_from.filter(|(nth, _)| self.writes >= *nth).map(|(_, failure)| failure);
        self.single_write_failures.get(&self.writes).copied().or(from)
    }

    fn mangles_this_write(&self) -> bool {
        self.mangle_every_write || self.single_mangled_writes.contains(&self.writes)
    }
}

#[derive(Clone, Default)]
pub struct MemoryStore {
    state: Arc<Mutex<State>>,
}

impl MemoryStore {
    pub fn with_files(files: impl IntoIterator<Item = (u16, Vec<u8>)>) -> Self {
        let store = Self::default();
        store.state().files.extend(files);
        store
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn insert(&self, id: u16, bytes: Vec<u8>) {
        self.state().files.insert(id, bytes);
    }

    pub fn files(&self) -> BTreeMap<u16, Vec<u8>> {
        self.state().files.clone()
    }

    pub fn property(&self, property: PropertyId) -> Option<u32> {
        self.state().properties.get(&property.0).copied()
    }

    pub fn put_property(&self, property: PropertyId, value: u32) {
        self.state().properties.insert(property.0, value);
    }

    pub fn refuse_create(&self, id: u16) {
        self.fail_create(id, Failure::Status(Status::InvalidArgument));
    }

    pub fn fail_create(&self, id: u16, failure: Failure) {
        self.state().create_failures.insert(id, failure);
    }

    pub fn fail_remove(&self, id: u16, failure: Failure) {
        self.state().remove_failures.insert(id, failure);
    }

    pub fn fail_set_property(&self, property: PropertyId, failure: Failure) {
        self.state().set_property_failures.insert(property.0, failure);
    }

    pub fn stop_failing(&self) {
        let mut state = self.state();
        state.create_failures.clear();
        state.remove_failures.clear();
        state.set_property_failures.clear();
        state.writes_failing_from = None;
        state.single_write_failures.clear();
        state.mangle_every_write = false;
        state.single_mangled_writes.clear();
    }

    pub fn fail_writes_from(&self, nth: usize, failure: Failure) {
        self.state().writes_failing_from = Some((nth, failure));
    }

    pub fn fail_write(&self, nth: usize, failure: Failure) {
        self.state().single_write_failures.insert(nth, failure);
    }

    pub fn mangle_writes(&self) {
        self.state().mangle_every_write = true;
    }

    pub fn mangle_write(&self, nth: usize) {
        self.state().single_mangled_writes.insert(nth);
    }

    pub fn operations(&self) -> Vec<Operation> {
        self.state().operations.clone()
    }
}

fn missing(command: Command) -> DeviceError {
    DeviceError::Status { command, status: Status::InvalidDeviceAddress }
}

impl FileStore for MemoryStore {
    fn read(&mut self, file: FileId) -> Result<Vec<u8>, DeviceError> {
        self.state().files.get(&file.0).cloned().ok_or_else(|| missing(Command::OpenFile))
    }

    fn write(&mut self, file: FileId, data: &[u8]) -> Result<(), DeviceError> {
        let mut state = self.state();
        state.writes += 1;
        state.operations.push(Operation::Write(file.0));
        if let Some(failure) = state.write_failure() {
            return Err(failure.error(Command::WriteBufferBegin));
        }
        let mut stored = data.to_vec();
        if state.mangles_this_write()
            && let Some(last) = stored.last_mut()
        {
            *last = !*last;
        }
        let existing = state.files.get_mut(&file.0).ok_or_else(|| missing(Command::OpenFile))?;
        *existing = stored;
        Ok(())
    }

    fn create(&mut self, file: FileId) -> Result<(), DeviceError> {
        let mut state = self.state();
        state.operations.push(Operation::Create(file.0));
        if let Some(failure) = state.create_failures.get(&file.0) {
            return Err(failure.error(Command::CreateFile));
        }
        if state.files.contains_key(&file.0) {
            return Err(Failure::Status(Status::InvalidArgument).error(Command::CreateFile));
        }
        state.files.insert(file.0, Vec::new());
        Ok(())
    }

    fn remove(&mut self, file: FileId) -> Result<(), DeviceError> {
        let mut state = self.state();
        state.operations.push(Operation::Remove(file.0));
        if let Some(failure) = state.remove_failures.get(&file.0) {
            return Err(failure.error(Command::RemoveFile));
        }
        state.files.remove(&file.0).map(drop).ok_or_else(|| missing(Command::RemoveFile))
    }

    fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError> {
        self.property(property)
            .ok_or(DeviceError::Status { command: Command::GetProperty, status: Status::PropertyNotSupported })
    }

    fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
        let mut state = self.state();
        state.operations.push(Operation::SetProperty(property.0, value));
        if let Some(failure) = state.set_property_failures.get(&property.0) {
            return Err(failure.error(Command::SetProperty));
        }
        state.properties.insert(property.0, value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(result: Result<impl std::fmt::Debug, DeviceError>) -> Option<Status> {
        result.unwrap_err().status()
    }

    #[test]
    fn clones_share_the_same_files() {
        let mut store = MemoryStore::with_files([(0x0F, vec![1])]);
        let handle = store.clone();
        store.create(FileId(0x6D60)).unwrap();
        store.write(FileId(0x6D60), &[7, 8]).unwrap();
        assert_eq!(handle.files()[&0x6D60], [7, 8]);
        assert_eq!(handle.operations(), [Operation::Create(0x6D60), Operation::Write(0x6D60)]);
    }

    #[test]
    fn missing_files_and_properties_report_the_device_statuses() {
        let mut store = MemoryStore::with_files([(0x6D60, vec![])]);
        assert_eq!(status(store.read(FileId(0x6D61))), Some(Status::InvalidDeviceAddress));
        assert_eq!(status(store.write(FileId(0x6D61), &[1])), Some(Status::InvalidDeviceAddress));
        assert_eq!(status(store.remove(FileId(0x6D61))), Some(Status::InvalidDeviceAddress));
        assert_eq!(status(store.create(FileId(0x6D60))), Some(Status::InvalidArgument));
        assert_eq!(status(store.get_property(PropertyId::FREE_STORAGE_SIZE)), Some(Status::PropertyNotSupported));
    }

    #[test]
    fn refused_and_failing_ids_fail_their_operation() {
        let mut store = MemoryStore::with_files([(0x6D62, vec![])]);
        store.refuse_create(0x6D60);
        store.fail_create(0x6D61, Failure::Timeout);
        store.fail_remove(0x6D62, Failure::Status(Status::HardwareError));
        assert_eq!(status(store.create(FileId(0x6D60))), Some(Status::InvalidArgument));
        assert!(matches!(store.create(FileId(0x6D61)), Err(DeviceError::Timeout(Command::CreateFile))));
        assert_eq!(status(store.remove(FileId(0x6D62))), Some(Status::HardwareError));
        assert!(store.files().contains_key(&0x6D62));
    }

    #[test]
    fn write_failures_and_mangling_count_write_calls_from_one() {
        let mut store = MemoryStore::with_files([(0x6D60, vec![])]);
        store.fail_write(2, Failure::Timeout);
        store.fail_writes_from(4, Failure::Status(Status::HardwareError));
        store.mangle_write(3);
        store.write(FileId(0x6D60), &[1]).unwrap();
        assert!(store.write(FileId(0x6D60), &[2]).is_err());
        store.write(FileId(0x6D60), &[0x0F]).unwrap();
        assert_eq!(store.files()[&0x6D60], [0xF0]);
        assert!(store.write(FileId(0x6D60), &[4]).is_err());
        assert!(store.write(FileId(0x6D60), &[5]).is_err());
    }

    #[test]
    fn mangled_writes_invert_the_last_byte() {
        let mut store = MemoryStore::with_files([(0x6D60, vec![])]);
        store.mangle_writes();
        store.write(FileId(0x6D60), &[1, 0x00]).unwrap();
        assert_eq!(store.files()[&0x6D60], [1, 0xFF]);
    }

    #[test]
    fn properties_are_read_and_set() {
        let mut store = MemoryStore::default();
        store.put_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 2);
        assert_eq!(store.get_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX).unwrap(), 2);
        store.set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 0).unwrap();
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(0));
        assert_eq!(store.operations(), [Operation::SetProperty(0x6E, 0)]);
    }

    #[test]
    fn a_failing_property_write_keeps_the_old_value_until_failures_stop() {
        let mut store = MemoryStore::with_files([(0x6D60, vec![])]);
        store.put_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 2);
        store.fail_set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, Failure::Timeout);
        store.fail_writes_from(1, Failure::Timeout);
        assert!(store.set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 0).is_err());
        assert!(store.write(FileId(0x6D60), &[1]).is_err());
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(2));
        store.stop_failing();
        store.set_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 0).unwrap();
        store.write(FileId(0x6D60), &[1]).unwrap();
        assert_eq!(store.property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX), Some(0));
    }
}
