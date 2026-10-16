#[cfg(target_os = "linux")]
pub mod linux_events;
#[cfg(windows)]
mod sendinput;
#[cfg(target_os = "linux")]
mod uinput;
#[cfg(any(windows, test))]
pub mod windows_inputs;

use crate::sink::{InputSink, SinkError};

#[cfg(windows)]
pub fn platform_sink() -> Result<Box<dyn InputSink>, SinkError> {
    Ok(Box::new(sendinput::SendInputSink))
}

#[cfg(target_os = "linux")]
pub fn platform_sink() -> Result<Box<dyn InputSink>, SinkError> {
    Ok(Box::new(uinput::UinputSink::new()?))
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn platform_sink() -> Result<Box<dyn InputSink>, SinkError> {
    Err(SinkError("key actions are not supported on this platform".into()))
}
