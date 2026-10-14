use std::time::Duration;

use bragi::REPORT_LEN;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("hid error: {0}")]
    Hid(String),
    #[error("device disconnected")]
    Disconnected,
}

pub trait Transport: Send {
    fn write(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), TransportError>;
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportError>;
}

pub trait InputSource: Send {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportError>;
}
