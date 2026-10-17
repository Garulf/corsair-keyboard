use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bragi::PropertyId;
use ipc::Profile;

use crate::{Backups, MemoryStore};

pub(crate) const BLOB_STAND_IN: [u8; 7] = [0x0B, 0xB1, 0x02, 0, 0, 0, 0];

macro_rules! fixture_files {
    ($($id:literal => $name:literal),* $(,)?) => {
        pub(crate) fn file(id: u16) -> &'static [u8] {
            match id {
                0x6D60 => &BLOB_STAND_IN,
                $($id => include_bytes!(concat!("../fixtures/", $name, ".bin")),)*
                _ => panic!("no fixture for file {id:#06x}"),
            }
        }

        pub(crate) const FIXTURE_IDS: &[u16] = &[0x6D60, $($id),*];
    };
}

fixture_files! {
    0x0F => "0f",
    0x25 => "25",
    0x6D61 => "6d61",
    0x6D62 => "6d62",
    0x6D63 => "6d63",
    0x6D64 => "6d64",
    0x6D65 => "6d65",
    0x6D66 => "6d66",
    0x6D67 => "6d67",
    0x6D68 => "6d68",
    0x6D69 => "6d69",
    0x6D6A => "6d6a",
    0x6D6B => "6d6b",
    0x6D6C => "6d6c",
    0x6D6D => "6d6d",
    0x6D6E => "6d6e",
    0x6D70 => "6d70",
}

pub(crate) fn dump_store() -> MemoryStore {
    let store = MemoryStore::with_files(FIXTURE_IDS.iter().map(|id| (*id, file(*id).to_vec())));
    store.put_property(PropertyId::ACTIVE_HARDWARE_PROFILE_INDEX, 0);
    store
}

pub(crate) fn red_profile() -> Profile {
    let text = r##"
        name = "red"

        [[layers]]
        name = "all"
        zone = "all"
        effect = { type = "solid", color = "#ff0000" }
    "##;
    toml::from_str(text).expect("the red profile parses")
}

pub(crate) fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_792_238_461)
}

pub(crate) fn rig() -> (MemoryStore, Backups, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("a temp dir");
    let backups = Backups::under(temp.path());
    (dump_store(), backups, temp)
}

#[test]
fn the_blob_file_is_replaced_by_the_stand_in() {
    assert_eq!(file(0x6D60), BLOB_STAND_IN);
}

#[test]
#[should_panic(expected = "no fixture")]
fn an_id_without_a_fixture_panics() {
    file(0x6D6F);
}
