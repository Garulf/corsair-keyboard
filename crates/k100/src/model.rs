pub const CORSAIR_VID: u16 = 0x1B1C;
pub const VENDOR_USAGE_PAGE: u16 = 0xFF42;
pub const COMMAND_USAGE: u16 = 0x01;
pub const INPUT_USAGE: u16 = 0x02;

const K100_KEY_COUNT: usize = 146;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub pid: u16,
    pub name: &'static str,
    pub tested: bool,
    pub key_count: usize,
}

pub const MODELS: &[Model] = &[
    Model {
        pid: 0x1BC5,
        name: "K100 RGB Optical-Mechanical",
        tested: true,
        key_count: K100_KEY_COUNT,
    },
    Model {
        pid: 0x1B7C,
        name: "K100 RGB Optical-Mechanical (OPX)",
        tested: false,
        key_count: K100_KEY_COUNT,
    },
    Model {
        pid: 0x1B7D,
        name: "K100 RGB Mechanical",
        tested: false,
        key_count: K100_KEY_COUNT,
    },
];

pub fn model_for_pid(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.pid == pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_known_models() {
        assert!(model_for_pid(0x1BC5).unwrap().tested);
        assert!(!model_for_pid(0x1B7C).unwrap().tested);
        assert!(model_for_pid(0x1BAB).is_none());
    }
}
