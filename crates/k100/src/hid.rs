use std::time::Duration;

use bragi::REPORT_LEN;
use hidapi::{HidApi, HidDevice};

use crate::model::{COMMAND_USAGE, CORSAIR_VID, INPUT_USAGE, Model, VENDOR_USAGE_PAGE, model_for_pid};
use crate::transport::{InputSource, Transport, TransportError};

const INPUT_REPORT_LEN: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("hid error: {0}")]
    Hid(#[from] hidapi::HidError),
    #[error("no supported Corsair K100 found")]
    NotFound,
    #[error("found the K100 but not its {0} interface")]
    MissingInterface(&'static str),
}

pub struct OpenedDevice {
    pub commands: HidTransport,
    pub input: HidInput,
    pub model: &'static Model,
    pub serial: String,
}

pub fn open_device() -> Result<OpenedDevice, OpenError> {
    let api = HidApi::new()?;
    let mut commands = None;
    let mut input = None;
    let mut found = None;
    let mut serial = String::new();
    for info in api.device_list() {
        if info.vendor_id() != CORSAIR_VID || info.usage_page() != VENDOR_USAGE_PAGE {
            continue;
        }
        let Some(model) = model_for_pid(info.product_id()) else {
            continue;
        };
        if found.is_some_and(|m: &Model| m.pid != model.pid) {
            continue;
        }
        match info.usage() {
            COMMAND_USAGE => {
                commands = Some(info.open_device(&api)?);
                serial = info.serial_number().unwrap_or_default().to_owned();
            }
            INPUT_USAGE => input = Some(info.open_device(&api)?),
            _ => continue,
        }
        found = Some(model);
    }
    let model = found.ok_or(OpenError::NotFound)?;
    Ok(OpenedDevice {
        commands: HidTransport(commands.ok_or(OpenError::MissingInterface("command"))?),
        input: HidInput(input.ok_or(OpenError::MissingInterface("input"))?),
        model,
        serial,
    })
}

pub struct HidTransport(HidDevice);

pub struct HidInput(HidDevice);

fn hid_error(error: hidapi::HidError) -> TransportError {
    TransportError::Hid(error.to_string())
}

fn read_report(device: &HidDevice, len: usize, timeout: Duration) -> Result<Option<Vec<u8>>, TransportError> {
    let mut buffer = vec![0u8; len];
    let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let read = device.read_timeout(&mut buffer, millis).map_err(hid_error)?;
    Ok((read > 0).then(|| {
        buffer.truncate(read);
        buffer
    }))
}

impl Transport for HidTransport {
    fn write(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), TransportError> {
        let mut buffer = [0u8; REPORT_LEN + 1];
        buffer[1..].copy_from_slice(report);
        self.0.write(&buffer).map(drop).map_err(hid_error)
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportError> {
        read_report(&self.0, REPORT_LEN, timeout)
    }
}

impl InputSource for HidInput {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportError> {
        read_report(&self.0, INPUT_REPORT_LEN, timeout)
    }
}
