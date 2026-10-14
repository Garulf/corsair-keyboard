mod device;
pub mod fake;
pub mod hid;
mod input;
mod keymap;
mod leds;
mod model;
mod session;
mod transport;

pub use device::{Device, DeviceError};
pub use input::{InputEvent, KeyStateTracker};
pub use keymap::{KeyMappings, Mapping};
pub use leds::{Frame, IndicatorRecord, LED_COUNT, Rgb, encode_frame, encode_indicators, led};
pub use model::{COMMAND_USAGE, CORSAIR_VID, INPUT_USAGE, MODELS, Model, VENDOR_USAGE_PAGE, model_for_pid};
pub use session::{DeviceInfo, PING_INTERVAL, Session, restore};
pub use transport::{InputSource, Transport, TransportError};
