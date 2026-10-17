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

    pub fn describe_buffer(&mut self, handle: u8) -> Result<Vec<u8>, DeviceError> {
        Ok(self.exchange(&Request::describe_buffer(handle))?.payload().to_vec())
    }

    pub fn read_buffer(&mut self, handle: u8) -> Result<Vec<u8>, DeviceError> {
        Ok(self.exchange(&Request::read_buffer(handle))?.payload().to_vec())
    }

    pub fn read_file(&mut self, handle: u8, file: FileId) -> Result<Vec<u8>, DeviceError> {
        self.open_file(handle, file)?;
        let contents = self.read_open_file(handle);
        let unbound = self.unbind(handle);
        let contents = contents?;
        unbound?;
        Ok(contents)
    }

    fn read_open_file(&mut self, handle: u8) -> Result<Vec<u8>, DeviceError> {
        let size = described_size(&self.describe_buffer(handle)?);
        let mut contents = Vec::with_capacity(size);
        while contents.len() < size {
            let chunk = self.read_buffer(handle)?;
            if chunk.is_empty() {
                break;
            }
            contents.extend(chunk);
        }
        contents.truncate(size);
        Ok(contents)
    }

    pub fn create_file(&mut self, file: FileId) -> Result<(), DeviceError> {
        self.exchange(&Request::create_file(file)).map(drop)
    }

    pub fn remove_file(&mut self, file: FileId) -> Result<(), DeviceError> {
        self.exchange(&Request::remove_file(file)).map(drop)
    }

    pub fn replace_file(&mut self, handle: u8, file: FileId, data: &[u8]) -> Result<(), DeviceError> {
        self.open_file(handle, file)?;
        let written = self.describe_buffer(handle).and_then(|_| self.write_file(handle, data));
        let unbound = self.unbind(handle);
        written?;
        unbound
    }

    pub fn ping(&mut self) -> Result<(), DeviceError> {
        self.exchange(&Request::ping()).map(drop)
    }
}

fn described_size(payload: &[u8]) -> usize {
    let mut size = [0u8; 4];
    if let Some(bytes) = payload.get(2..6) {
        size.copy_from_slice(bytes);
    }
    u32::from_le_bytes(size) as usize
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

    #[test]
    fn read_and_describe_buffer_return_the_reply_payload() {
        let fake = FakeTransport::with_responder(|request| {
            let mut reply = ok_reply(request);
            reply.extend_from_slice(&[request[1], 0xAB]);
            vec![reply]
        });
        let log = fake.log();
        let mut device = Device::new(fake);
        assert_eq!(&device.describe_buffer(4).unwrap()[..2], [0x09, 0xAB]);
        assert_eq!(&device.read_buffer(4).unwrap()[..2], [0x08, 0xAB]);
        let log = log.lock().unwrap();
        assert_eq!([log[0][2], log[1][2]], [4, 4]);
    }

    fn file_responder(contents: Vec<u8>) -> impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static {
        let mut offset = 0;
        move |request| {
            let mut reply = ok_reply(request);
            match request[1] {
                0x09 => reply.extend_from_slice(&[0, 0, contents.len() as u8, 0, 0, 0]),
                0x08 => {
                    let end = (offset + 1021).min(contents.len());
                    reply.extend_from_slice(&contents[offset..end]);
                    reply.resize(1024, 0);
                    offset = end;
                }
                _ => {}
            }
            vec![reply]
        }
    }

    #[test]
    fn read_file_reads_the_described_size_and_unbinds() {
        let contents: Vec<u8> = (0..200u8).collect();
        let fake = FakeTransport::with_responder(file_responder(contents.clone()));
        let log = fake.log();
        let mut device = Device::new(fake);
        assert_eq!(device.read_file(3, FileId(0x6D70)).unwrap(), contents);
        let commands: Vec<u8> = log.lock().unwrap().iter().map(|report| report[1]).collect();
        assert_eq!(commands, [0x0D, 0x09, 0x08, 0x05]);
    }

    #[test]
    fn read_file_of_an_empty_file_sends_no_read() {
        let fake = FakeTransport::with_responder(file_responder(Vec::new()));
        let log = fake.log();
        let mut device = Device::new(fake);
        assert!(device.read_file(3, FileId(0x0F)).unwrap().is_empty());
        let commands: Vec<u8> = log.lock().unwrap().iter().map(|report| report[1]).collect();
        assert_eq!(commands, [0x0D, 0x09, 0x05]);
    }

    #[test]
    fn read_file_unbinds_even_when_a_read_fails() {
        let fake = FakeTransport::with_responder(|request| match request[1] {
            0x08 => vec![status_reply(request, 3)],
            0x09 => {
                let mut reply = ok_reply(request);
                reply.extend_from_slice(&[0, 0, 10, 0, 0, 0]);
                vec![reply]
            }
            _ => vec![ok_reply(request)],
        });
        let log = fake.log();
        let mut device = Device::new(fake);
        assert!(device.read_file(3, FileId(0x11)).is_err());
        assert_eq!(log.lock().unwrap().last().unwrap()[1], 0x05);
    }

    #[test]
    fn read_file_stops_when_reads_return_nothing() {
        let fake = FakeTransport::with_responder(|request| {
            let mut reply = ok_reply(request);
            if request[1] == 0x09 {
                reply.extend_from_slice(&[0, 0, 10, 0, 0, 0]);
            }
            vec![reply]
        });
        let mut device = Device::new(fake);
        assert!(device.read_file(3, FileId(0x25)).unwrap().is_empty());
    }

    #[test]
    fn replace_file_opens_describes_writes_and_unbinds() {
        let fake = FakeTransport::new();
        let log = fake.log();
        let mut device = Device::new(fake);
        device.replace_file(3, FileId(0x0F), &[0x13, 0x00, 0x00, 0x00]).unwrap();
        let log = log.lock().unwrap();
        let commands: Vec<u8> = log.iter().map(|report| report[1]).collect();
        assert_eq!(commands, [0x0D, 0x09, 0x06, 0x05]);
        assert_eq!(&log[2][2..11], [3, 4, 0, 0, 0, 0x13, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn replace_file_unbinds_when_the_write_fails() {
        let fake = FakeTransport::with_responder(|request| match request[1] {
            0x06 => vec![status_reply(request, 3)],
            _ => vec![ok_reply(request)],
        });
        let log = fake.log();
        let mut device = Device::new(fake);
        assert!(device.replace_file(3, FileId(0x6D71), &[1, 2]).is_err());
        assert_eq!(log.lock().unwrap().last().unwrap()[1], 0x05);
    }

    #[test]
    fn create_and_remove_file_send_one_command_each() {
        let fake = FakeTransport::new();
        let log = fake.log();
        let mut device = Device::new(fake);
        device.create_file(FileId(0x6D71)).unwrap();
        device.remove_file(FileId(0x6D71)).unwrap();
        let commands: Vec<u8> = log.lock().unwrap().iter().map(|report| report[1]).collect();
        assert_eq!(commands, [0x0B, 0x0C]);
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
