use std::path::PathBuf;

use k100::DeviceError;

use crate::DecodeError;

#[derive(Debug, thiserror::Error)]
pub enum OnboardError {
    #[error("keyboard: {0}")]
    Device(#[from] DeviceError),
    #[error("file {0:#06x} on the keyboard could not be decoded: {1}")]
    Decode(u16, DecodeError),
    #[error("there is no onboard slot {0}")]
    NoSlot(usize),
    #[error("the only onboard slot cannot be deleted")]
    LastSlot,
    #[error("the keyboard has no free file ids left")]
    NoFreeIds,
    #[error("file {0:#06x} read back different from what was written")]
    Verify(u16),
    #[error("file {0:#06x} is outside the free-use range 0x6d60..0x7fff")]
    NotFreeUse(u16),
    #[error("{} is not an onboard backup", .0.display())]
    NotABackup(PathBuf),
    #[error("no profile named {0:?}")]
    NoProfile(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl OnboardError {
    pub fn lost_device(&self) -> bool {
        matches!(self, Self::Device(error) if error.status().is_none())
    }
}

#[cfg(test)]
mod tests {
    use bragi::{Command, Status};

    use super::*;

    #[test]
    fn only_a_device_error_without_a_status_means_the_keyboard_is_gone() {
        assert!(OnboardError::Device(DeviceError::Timeout(Command::CreateFile)).lost_device());
        let refused = DeviceError::Status { command: Command::CreateFile, status: Status::InvalidArgument };
        assert!(!OnboardError::Device(refused).lost_device());
        assert!(!OnboardError::Verify(0x6D71).lost_device());
    }

    #[test]
    fn messages_name_the_file_in_hex() {
        assert_eq!(OnboardError::Verify(0x6D71).to_string(), "file 0x6d71 read back different from what was written");
        assert_eq!(OnboardError::NotFreeUse(2).to_string(), "file 0x0002 is outside the free-use range 0x6d60..0x7fff");
    }
}
