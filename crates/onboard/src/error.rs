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
    #[error("{error}; left on the keyboard: {}", hex_ids(.leftovers))]
    PartlyUndone { error: Box<OnboardError>, leftovers: Vec<u16> },
}

fn hex_ids(ids: &[u16]) -> String {
    ids.iter().map(|id| format!("{id:#06x}")).collect::<Vec<_>>().join(", ")
}

impl OnboardError {
    pub fn lost_device(&self) -> bool {
        self.device().is_some_and(|error| error.status().is_none())
    }

    pub fn device(&self) -> Option<&DeviceError> {
        match self {
            Self::Device(error) => Some(error),
            Self::PartlyUndone { error, .. } => error.device(),
            _ => None,
        }
    }

    pub fn leftovers(&self) -> &[u16] {
        match self {
            Self::PartlyUndone { leftovers, .. } => leftovers,
            _ => &[],
        }
    }

    pub(crate) fn with_leftovers(self, leftovers: Vec<u16>) -> Self {
        if leftovers.is_empty() { self } else { Self::PartlyUndone { error: Box::new(self), leftovers } }
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
    fn a_partly_undone_change_keeps_the_cause_and_names_what_it_left() {
        let cause = OnboardError::Device(DeviceError::Timeout(Command::WriteBufferBegin));
        let error = OnboardError::PartlyUndone { error: Box::new(cause), leftovers: vec![0x6D6F, 0x6D71] };
        assert!(error.lost_device());
        assert!(matches!(error.device(), Some(DeviceError::Timeout(Command::WriteBufferBegin))));
        assert!(OnboardError::Verify(0x6D71).device().is_none());
        assert_eq!(error.leftovers(), [0x6D6F, 0x6D71]);
        assert!(error.to_string().ends_with("; left on the keyboard: 0x6d6f, 0x6d71"), "{error}");
        assert!(OnboardError::LastSlot.leftovers().is_empty());
    }

    #[test]
    fn messages_name_the_file_in_hex() {
        assert_eq!(OnboardError::Verify(0x6D71).to_string(), "file 0x6d71 read back different from what was written");
        assert_eq!(OnboardError::NotFreeUse(2).to_string(), "file 0x0002 is outside the free-use range 0x6d60..0x7fff");
    }
}
