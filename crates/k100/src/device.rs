use std::time::{Duration, Instant};

use bragi::{Command, FileId, PropertyId, Request, Response, ResponseError, Status, write_buffer_packets};

use crate::transport::{Transport, TransportError};

const RESPONSE_TIMEOUT: Duration = Duration::from_millis(1000);
const MAX_DRAINED_REPORTS: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("no reply to {0:?}")]
    Timeout(Command),
    #[error("{command:?} failed: {status}")]
    Status { command: Command, status: Status },
    #[error("malformed reply: {0}")]
    Malformed(#[from] ResponseError),
}

impl DeviceError {
    pub fn status(&self) -> Option<Status> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub struct Device<T> {
    transport: T,
}

impl<T: Transport> Device<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn exchange(&mut self, request: &Request) -> Result<Response, DeviceError> {
        self.drain()?;
        self.transport.write(request.as_bytes())?;
        let command = request.command();
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
            let Some(bytes) = self.transport.read(remaining)? else {
                break;
            };
            if bytes.get(1) != Some(&(command as u8)) {
                continue;
            }
            let response = match Response::parse(&bytes) {
                Ok(response) => response,
                Err(ResponseError::NotAReply | ResponseError::TooShort(_)) => continue,
                Err(error) => return Err(error.into()),
            };
            response.status().map_err(|status| DeviceError::Status { command, status })?;
            return Ok(response);
        }
        Err(DeviceError::Timeout(command))
    }

    fn drain(&mut self) -> Result<(), DeviceError> {
        for _ in 0..MAX_DRAINED_REPORTS {
            if self.transport.read(Duration::ZERO)?.is_none() {
                break;
            }
        }
        Ok(())
    }

    pub fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError> {
        Ok(self.exchange(&Request::get_property(property))?.value_u32())
    }

    pub fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
        self.exchange(&Request::set_property(property, value)).map(drop)
    }

    pub fn open_file(&mut self, handle: u8, file: FileId) -> Result<(), DeviceError> {
        self.exchange(&Request::open_file(handle, file)).map(drop)
    }

    pub fn unbind(&mut self, handle: u8) -> Result<(), DeviceError> {
        self.exchange(&Request::unbind_buffers(handle)).map(drop)
    }

    pub fn write_file(&mut self, handle: u8, data: &[u8]) -> Result<(), DeviceError> {
        for packet in write_buffer_packets(handle, data) {
            self.exchange(&packet)?;
        }
        Ok(())
    }

    pub fn ping(&mut self) -> Result<(), DeviceError> {
        self.exchange(&Request::ping()).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeTransport, ok_reply, status_reply, value_reply};
    use bragi::{FileId, PropertyId};

    #[test]
    fn get_property_returns_value() {
        let fake = FakeTransport::with_responder(|request| vec![value_reply(request, 0x0020_0103)]);
        let mut device = Device::new(fake);
        assert_eq!(device.get_property(PropertyId::FIRMWARE_VERSION).unwrap(), 0x0020_0103);
    }

    #[test]
    fn skips_replies_for_other_commands() {
        let fake = FakeTransport::with_responder(|request| vec![vec![0x00, 0x12, 0x00], value_reply(request, 5)]);
        let mut device = Device::new(fake);
        assert_eq!(device.get_property(PropertyId::BRIGHTNESS).unwrap(), 5);
    }

    #[test]
    fn drains_stale_replies_before_writing() {
        let mut fake = FakeTransport::with_responder(|request| vec![value_reply(request, 2)]);
        fake.push_unsolicited(value_reply(&Request::get_property(PropertyId::OPERATING_MODE).as_bytes()[..], 1));
        let mut device = Device::new(fake);
        assert_eq!(device.get_property(PropertyId::OPERATING_MODE).unwrap(), 2);
    }

    #[test]
    fn times_out_without_reply() {
        let fake = FakeTransport::with_responder(|_| vec![]);
        let mut device = Device::new(fake);
        assert!(matches!(device.ping(), Err(DeviceError::Timeout(Command::Ping))));
    }

    #[test]
    fn maps_status_errors() {
        let fake = FakeTransport::with_responder(|request| vec![status_reply(request, 5)]);
        let mut device = Device::new(fake);
        let error = device.get_property(PropertyId::MAXIMUM_POLLING_RATE).unwrap_err();
        assert_eq!(error.status(), Some(Status::PropertyNotSupported));
    }

    #[test]
    fn write_file_sends_every_packet() {
        let fake = FakeTransport::new();
        let log = fake.log();
        let mut device = Device::new(fake);
        device.open_file(0, FileId::REAL_TIME_LIGHTINGS_BUFFER).unwrap();
        device.write_file(0, &[7u8; 1500]).unwrap();
        let log = log.lock().unwrap();
        let commands: Vec<u8> = log.iter().map(|report| report[1]).collect();
        assert_eq!(commands, [0x0D, 0x06, 0x07]);
        assert_eq!(ok_reply(&log[0])[1], 0x0D);
    }

    struct Chatty;

    impl Transport for Chatty {
        fn write(&mut self, _report: &[u8; bragi::REPORT_LEN]) -> Result<(), TransportError> {
            Ok(())
        }

        fn read(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, TransportError> {
            Ok(Some(vec![0x00, 0x02, 0x00]))
        }
    }

    #[test]
    fn stops_at_the_deadline_when_unrelated_reports_keep_arriving() {
        let started = Instant::now();
        let mut device = Device::new(Chatty);
        assert!(matches!(device.ping(), Err(DeviceError::Timeout(Command::Ping))));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn skips_transport_errors_addressed_to_other_commands() {
        let fake = FakeTransport::with_responder(|request| vec![vec![0x10, 0x02, 0x00], ok_reply(request)]);
        let mut device = Device::new(fake);
        device.ping().unwrap();
    }
}
