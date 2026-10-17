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

#[test]
fn the_blob_file_is_replaced_by_the_stand_in() {
    assert_eq!(file(0x6D60), BLOB_STAND_IN);
}

#[test]
#[should_panic(expected = "no fixture")]
fn an_id_without_a_fixture_panics() {
    file(0x6D6F);
}
