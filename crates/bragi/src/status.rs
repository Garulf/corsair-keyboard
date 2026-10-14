#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Status {
    #[error("invalid argument")]
    InvalidArgument,
    #[error("insufficient buffer size")]
    InsufficientBufferSize,
    #[error("invalid state")]
    InvalidState,
    #[error("command not supported")]
    CommandNotSupported,
    #[error("property not supported")]
    PropertyNotSupported,
    #[error("invalid device address")]
    InvalidDeviceAddress,
    #[error("hardware error")]
    HardwareError,
    #[error("unknown status {0:#04x}")]
    Unknown(u8),
}

impl Status {
    pub fn check(byte: u8) -> Result<(), Status> {
        Err(match byte {
            0 => return Ok(()),
            1 => Self::InvalidArgument,
            2 => Self::InsufficientBufferSize,
            3 => Self::InvalidState,
            4 => Self::CommandNotSupported,
            5 => Self::PropertyNotSupported,
            6 => Self::InvalidDeviceAddress,
            7 => Self::HardwareError,
            other => Self::Unknown(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_status_bytes() {
        assert_eq!(Status::check(0), Ok(()));
        assert_eq!(Status::check(3), Err(Status::InvalidState));
        assert_eq!(Status::check(6), Err(Status::InvalidDeviceAddress));
        assert_eq!(Status::check(0x42), Err(Status::Unknown(0x42)));
    }
}
