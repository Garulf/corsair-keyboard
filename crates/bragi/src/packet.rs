use crate::{Command, FileId, PropertyId};

pub const REPORT_LEN: usize = 1024;

const REQUEST_FLAG: u8 = 0x08;
const UNBIND_SUBCOMMAND: u8 = 0x01;
const BEGIN_HEADER_LEN: usize = 7;
const CONTINUE_HEADER_LEN: usize = 3;

#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    command: Command,
    bytes: Box<[u8; REPORT_LEN]>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let used = self.bytes.iter().rposition(|&b| b != 0).map_or(2, |i| i + 1);
        write!(f, "Request({:02x?})", &self.bytes[..used])
    }
}

impl Request {
    fn new(command: Command, payload: &[u8]) -> Self {
        let mut bytes = Box::new([0u8; REPORT_LEN]);
        bytes[0] = REQUEST_FLAG;
        bytes[1] = command as u8;
        bytes[2..2 + payload.len()].copy_from_slice(payload);
        Self { command, bytes }
    }

    pub fn set_property(property: PropertyId, value: u32) -> Self {
        let mut payload = [0u8; 6];
        payload[..2].copy_from_slice(&property.0.to_le_bytes());
        payload[2..].copy_from_slice(&value.to_le_bytes());
        Self::new(Command::SetProperty, &payload)
    }

    pub fn get_property(property: PropertyId) -> Self {
        Self::new(Command::GetProperty, &property.0.to_le_bytes())
    }

    pub fn open_file(handle: u8, file: FileId) -> Self {
        let [lo, hi] = file.0.to_le_bytes();
        Self::new(Command::OpenFile, &[handle, lo, hi])
    }

    pub fn unbind_buffers(handle: u8) -> Self {
        Self::new(Command::UnbindBuffers, &[UNBIND_SUBCOMMAND, handle])
    }

    pub fn ping() -> Self {
        Self::new(Command::Ping, &[])
    }

    pub fn command(&self) -> Command {
        self.command
    }

    pub fn as_bytes(&self) -> &[u8; REPORT_LEN] {
        &self.bytes
    }
}

pub fn write_buffer_packets(handle: u8, data: &[u8]) -> Vec<Request> {
    let begin_capacity = REPORT_LEN - BEGIN_HEADER_LEN;
    let (first, rest) = data.split_at(data.len().min(begin_capacity));

    let mut begin = Vec::with_capacity(5 + first.len());
    begin.push(handle);
    begin.extend_from_slice(&(data.len() as u32).to_le_bytes());
    begin.extend_from_slice(first);

    let mut packets = vec![Request::new(Command::WriteBufferBegin, &begin)];
    for chunk in rest.chunks(REPORT_LEN - CONTINUE_HEADER_LEN) {
        let mut payload = Vec::with_capacity(1 + chunk.len());
        payload.push(handle);
        payload.extend_from_slice(chunk);
        packets.push(Request::new(Command::WriteBuffer, &payload));
    }
    packets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileId, PropertyId};

    fn prefix(request: &Request, len: usize) -> &[u8] {
        &request.as_bytes()[..len]
    }

    fn tail_is_zero(request: &Request, from: usize) -> bool {
        request.as_bytes()[from..].iter().all(|&b| b == 0)
    }

    #[test]
    fn set_property_encodes_u16_id_and_u32_value() {
        let request = Request::set_property(PropertyId::BRIGHTNESS, 1000);
        assert_eq!(prefix(&request, 8), [0x08, 0x01, 0x02, 0x00, 0xE8, 0x03, 0x00, 0x00]);
        assert!(tail_is_zero(&request, 8));
        assert_eq!(request.command(), Command::SetProperty);
    }

    #[test]
    fn get_property_encodes_wide_property_ids() {
        let request = Request::get_property(PropertyId::BRAGI_PROTOCOL_VERSION);
        assert_eq!(prefix(&request, 4), [0x08, 0x02, 0x05, 0x01]);
        assert!(tail_is_zero(&request, 4));
    }

    #[test]
    fn open_file_encodes_handle_and_file_id() {
        let request = Request::open_file(1, FileId::INDICATION_REAL_TIME_LIGHTINGS_BUFFER);
        assert_eq!(prefix(&request, 5), [0x08, 0x0D, 0x01, 0x2E, 0x00]);
    }

    #[test]
    fn unbind_and_ping() {
        assert_eq!(prefix(&Request::unbind_buffers(2), 4), [0x08, 0x05, 0x01, 0x02]);
        assert_eq!(prefix(&Request::ping(), 2), [0x08, 0x12]);
        assert!(tail_is_zero(&Request::ping(), 2));
    }

    #[test]
    fn write_fits_in_one_packet() {
        let data = [0xAAu8; 581];
        let packets = write_buffer_packets(0, &data);
        assert_eq!(packets.len(), 1);
        assert_eq!(prefix(&packets[0], 7), [0x08, 0x06, 0x00, 0x45, 0x02, 0x00, 0x00]);
        assert_eq!(&packets[0].as_bytes()[7..588], &data[..]);
        assert!(tail_is_zero(&packets[0], 588));
    }

    #[test]
    fn write_splits_at_report_boundary_with_real_handle() {
        let data: Vec<u8> = (0..1500u32).map(|i| (i % 251) as u8).collect();
        let packets = write_buffer_packets(3, &data);
        assert_eq!(packets.len(), 2);
        assert_eq!(prefix(&packets[0], 7), [0x08, 0x06, 0x03, 0xDC, 0x05, 0x00, 0x00]);
        assert_eq!(&packets[0].as_bytes()[7..], &data[..1017]);
        assert_eq!(prefix(&packets[1], 3), [0x08, 0x07, 0x03]);
        assert_eq!(&packets[1].as_bytes()[3..3 + 483], &data[1017..]);
        assert!(tail_is_zero(&packets[1], 3 + 483));
        assert_eq!(packets[1].command(), Command::WriteBuffer);
    }

    #[test]
    fn write_exactly_one_begin_capacity_needs_no_continuation() {
        let packets = write_buffer_packets(0, &[1u8; 1017]);
        assert_eq!(packets.len(), 1);
    }

    #[test]
    fn empty_write_is_a_single_begin_packet() {
        let packets = write_buffer_packets(0, &[]);
        assert_eq!(packets.len(), 1);
        assert_eq!(prefix(&packets[0], 7), [0x08, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }
}
