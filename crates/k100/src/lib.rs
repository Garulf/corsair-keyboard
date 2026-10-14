mod device;
pub mod fake;
mod transport;

pub use device::{Device, DeviceError};
pub use transport::{InputSource, Transport, TransportError};
