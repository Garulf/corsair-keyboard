use std::time::Duration;

use bragi::{FileId, OperatingMode, PollingRate, PropertyId, Status};

use crate::device::{Device, DeviceError};
use crate::keymap::KeyMappings;
use crate::leds::{Frame, IndicatorRecord, encode_frame, encode_indicators};
use crate::model::Model;
use crate::transport::Transport;

pub const PING_INTERVAL: Duration = Duration::from_secs(24);

const LIGHTING_HANDLE: u8 = 0;
const INDICATION_HANDLE: u8 = 1;
const KEYMAP_HANDLE: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub pid: u16,
    pub firmware: u32,
    pub layout: u32,
    pub max_polling_rate: Option<PollingRate>,
    pub protocol_version: Option<u32>,
}

pub struct Session<T: Transport> {
    device: Device<T>,
    model: &'static Model,
    info: DeviceInfo,
    released: bool,
}

impl<T: Transport> Session<T> {
    pub fn start(transport: T, model: &'static Model) -> Result<Self, DeviceError> {
        let mut device = Device::new(transport);
        let info = read_info(&mut device)?;
        device.set_property(PropertyId::OPERATING_MODE, OperatingMode::Software.raw())?;
        let mut session = Self {
            device,
            model,
            info,
            released: false,
        };
        session
            .device
            .open_file(LIGHTING_HANDLE, FileId::REAL_TIME_LIGHTINGS_BUFFER)?;
        session
            .device
            .open_file(INDICATION_HANDLE, FileId::INDICATION_REAL_TIME_LIGHTINGS_BUFFER)?;
        Ok(session)
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn model(&self) -> &'static Model {
        self.model
    }

    pub fn write_frame(&mut self, frame: &Frame) -> Result<(), DeviceError> {
        self.write_lighting(
            LIGHTING_HANDLE,
            FileId::REAL_TIME_LIGHTINGS_BUFFER,
            &encode_frame(frame),
        )
    }

    pub fn write_indicators(&mut self, records: &[IndicatorRecord]) -> Result<(), DeviceError> {
        let data = encode_indicators(records);
        self.write_lighting(INDICATION_HANDLE, FileId::INDICATION_REAL_TIME_LIGHTINGS_BUFFER, &data)
    }

    pub fn set_key_mappings(&mut self, mappings: &KeyMappings) -> Result<(), DeviceError> {
        self.device.open_file(KEYMAP_HANDLE, FileId::KEY_MAPPINGS)?;
        let written = self.device.write_file(KEYMAP_HANDLE, mappings.as_bytes());
        let unbound = self.device.unbind(KEYMAP_HANDLE);
        written.and(unbound)
    }

    pub fn get_property(&mut self, property: PropertyId) -> Result<u32, DeviceError> {
        self.device.get_property(property)
    }

    pub fn set_property(&mut self, property: PropertyId, value: u32) -> Result<(), DeviceError> {
        self.device.set_property(property, value)
    }

    pub fn ping(&mut self) -> Result<(), DeviceError> {
        self.device.ping()
    }

    pub fn release(&mut self) -> Result<(), DeviceError> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        let keys = self.set_key_mappings(&KeyMappings::all_normal(self.model.key_count));
        let lighting = self.device.unbind(LIGHTING_HANDLE);
        let indication = self.device.unbind(INDICATION_HANDLE);
        let mode = self
            .device
            .set_property(PropertyId::OPERATING_MODE, OperatingMode::Hardware.raw());
        keys.and(lighting).and(indication).and(mode)
    }

    fn write_lighting(&mut self, handle: u8, file: FileId, data: &[u8]) -> Result<(), DeviceError> {
        match self.device.write_file(handle, data) {
            Ok(()) => Ok(()),
            Err(error) => {
                if matches!(
                    error.status(),
                    Some(Status::InvalidState | Status::InvalidDeviceAddress)
                ) {
                    let _ = self.device.unbind(handle);
                    self.device.open_file(handle, file)?;
                }
                self.device.write_file(handle, data)
            }
        }
    }
}

impl<T: Transport> Drop for Session<T> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn read_info<T: Transport>(device: &mut Device<T>) -> Result<DeviceInfo, DeviceError> {
    let firmware = device.get_property(PropertyId::FIRMWARE_VERSION)?;
    let pid = device.get_property(PropertyId::USB_PID)? as u16;
    let layout = device.get_property(PropertyId::PHYSICAL_KEYBOARD_LAYOUT)?;
    let max_polling_rate =
        optional(device.get_property(PropertyId::MAXIMUM_POLLING_RATE))?.and_then(PollingRate::from_raw);
    let protocol_version = optional(device.get_property(PropertyId::BRAGI_PROTOCOL_VERSION))?;
    Ok(DeviceInfo {
        pid,
        firmware,
        layout,
        max_polling_rate,
        protocol_version,
    })
}

fn optional(result: Result<u32, DeviceError>) -> Result<Option<u32>, DeviceError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if matches!(
                error.status(),
                Some(Status::PropertyNotSupported | Status::InvalidArgument)
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::fake::{FakeTransport, ok_reply, status_reply, value_reply};
    use crate::leds::{LED_COUNT, Rgb};
    use crate::model::model_for_pid;

    fn property_of(report: &[u8]) -> u16 {
        u16::from_le_bytes([report[2], report[3]])
    }

    fn healthy_device() -> FakeTransport {
        FakeTransport::with_responder(|request| {
            vec![match (request[1], property_of(request)) {
                (0x02, 0x13) => value_reply(request, 0x0103_0020),
                (0x02, 0x12) => value_reply(request, 0x1BC5),
                (0x02, 0x41) => value_reply(request, 1),
                (0x02, 0x96) => value_reply(request, 7),
                (0x02, 0x105) => value_reply(request, 0),
                _ => ok_reply(request),
            }]
        })
    }

    fn k100() -> &'static Model {
        model_for_pid(0x1BC5).unwrap()
    }

    fn commands(log: &Arc<Mutex<Vec<Vec<u8>>>>) -> Vec<(u8, u8)> {
        log.lock().unwrap().iter().map(|r| (r[1], r[2])).collect()
    }

    #[test]
    fn start_reads_info_then_enters_software_mode() {
        let fake = healthy_device();
        let log = fake.log();
        let mut session = Session::start(fake, k100()).unwrap();
        assert_eq!(session.info().pid, 0x1BC5);
        assert_eq!(session.info().max_polling_rate, Some(PollingRate::Hz8000));
        assert_eq!(
            commands(&log),
            [
                (0x02, 0x13),
                (0x02, 0x12),
                (0x02, 0x41),
                (0x02, 0x96),
                (0x02, 0x05),
                (0x01, 0x03),
                (0x0D, 0),
                (0x0D, 1)
            ]
        );
        let set_mode = log.lock().unwrap()[5].clone();
        assert_eq!(&set_mode[4..8], &[2, 0, 0, 0]);
        session.release().unwrap();
    }

    #[test]
    fn optional_properties_may_be_unsupported() {
        let fake = FakeTransport::with_responder(|request| {
            vec![match (request[1], property_of(request)) {
                (0x02, 0x96 | 0x105) => status_reply(request, 5),
                (0x02, _) => value_reply(request, 1),
                _ => ok_reply(request),
            }]
        });
        let session = Session::start(fake, k100()).unwrap();
        assert_eq!(session.info().max_polling_rate, None);
        assert_eq!(session.info().protocol_version, None);
    }

    #[test]
    fn frame_goes_to_the_lighting_handle() {
        let fake = healthy_device();
        let log = fake.log();
        let mut session = Session::start(fake, k100()).unwrap();
        log.lock().unwrap().clear();
        session.write_frame(&[Rgb::new(9, 8, 7); LED_COUNT]).unwrap();
        let log = log.lock().unwrap();
        assert_eq!(
            &log[0][..10],
            &[0x08, 0x06, LIGHTING_HANDLE, 0x45, 0x02, 0x00, 0x00, 0x12, 0x00, 9]
        );
    }

    #[test]
    fn invalid_state_reopens_the_file_and_retries_once() {
        let mut failed = false;
        let fake = FakeTransport::with_responder(move |request| {
            vec![if request[1] == 0x06 && !failed {
                failed = true;
                status_reply(request, 3)
            } else if request[1] == 0x02 {
                value_reply(request, 1)
            } else {
                ok_reply(request)
            }]
        });
        let log = fake.log();
        let mut session = Session::start(fake, k100()).unwrap();
        log.lock().unwrap().clear();
        session.write_frame(&[Rgb::BLACK; LED_COUNT]).unwrap();
        assert_eq!(commands(&log), [(0x06, 0), (0x05, 1), (0x0D, 0), (0x06, 0)]);
    }

    #[test]
    fn persistent_failure_surfaces_after_one_retry() {
        let fake = FakeTransport::with_responder(|request| {
            vec![match request[1] {
                0x06 => status_reply(request, 7),
                0x02 => value_reply(request, 1),
                _ => ok_reply(request),
            }]
        });
        let mut session = Session::start(fake, k100()).unwrap();
        let error = session.write_frame(&[Rgb::BLACK; LED_COUNT]).unwrap_err();
        assert_eq!(error.status(), Some(bragi::Status::HardwareError));
    }

    #[test]
    fn key_mappings_use_a_temporary_handle() {
        let fake = healthy_device();
        let log = fake.log();
        let mut session = Session::start(fake, k100()).unwrap();
        log.lock().unwrap().clear();
        session
            .set_key_mappings(&KeyMappings::all_normal(k100().key_count))
            .unwrap();
        let log = log.lock().unwrap();
        assert_eq!(&log[0][..5], &[0x08, 0x0D, KEYMAP_HANDLE, 0x02, 0x00]);
        assert_eq!(log[1][1], 0x06);
        assert_eq!(&log[2][..4], &[0x08, 0x05, 0x01, KEYMAP_HANDLE]);
    }

    #[test]
    fn release_restores_keys_closes_buffers_and_returns_to_hardware_mode() {
        let fake = healthy_device();
        let log = fake.log();
        let mut session = Session::start(fake, k100()).unwrap();
        log.lock().unwrap().clear();
        session.release().unwrap();
        session.release().unwrap();
        let log = log.lock().unwrap();
        let tail: Vec<(u8, u8)> = log.iter().map(|r| (r[1], r[2])).collect();
        assert_eq!(
            tail,
            [
                (0x0D, KEYMAP_HANDLE),
                (0x06, KEYMAP_HANDLE),
                (0x05, 1),
                (0x05, 1),
                (0x05, 1),
                (0x01, 0x03)
            ]
        );
        assert_eq!(&log[5][4..8], &[1, 0, 0, 0]);
    }

    #[test]
    fn drop_releases_even_after_a_panic() {
        let fake = healthy_device();
        let log = fake.log();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _session = Session::start(fake, k100()).unwrap();
            panic!("boom");
        }));
        assert!(result.is_err());
        let log = log.lock().unwrap();
        let last = log.last().unwrap();
        assert_eq!(&last[..8], &[0x08, 0x01, 0x03, 0x00, 1, 0, 0, 0]);
    }
}
