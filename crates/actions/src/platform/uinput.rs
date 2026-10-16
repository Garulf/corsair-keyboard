use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, InputEvent, KeyCode, RelativeAxisCode};

use super::linux_events::{BTN_EXTRA, BTN_LEFT, REL_HWHEEL, REL_WHEEL, REL_X, REL_Y, linux_events};
use crate::keys::all;
use crate::sink::{InputSink, SinkError, SinkEvent};

pub struct UinputSink {
    device: VirtualDevice,
}

impl UinputSink {
    pub fn new() -> Result<Self, SinkError> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for output in all().iter().filter_map(|def| def.output) {
            keys.insert(KeyCode::new(output.evdev));
        }
        for button in BTN_LEFT..=BTN_EXTRA {
            keys.insert(KeyCode::new(button));
        }
        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        for axis in [REL_X, REL_Y, REL_WHEEL, REL_HWHEEL] {
            axes.insert(RelativeAxisCode(axis));
        }
        let device = VirtualDevice::builder()
            .and_then(|builder| builder.name("k100d virtual input").with_keys(&keys))
            .and_then(|builder| builder.with_relative_axes(&axes))
            .and_then(|builder| builder.build())
            .map_err(|error| SinkError(format!("cannot create a virtual input device on /dev/uinput: {error}")))?;
        Ok(Self { device })
    }
}

impl InputSink for UinputSink {
    fn send(&mut self, event: &SinkEvent) -> Result<(), SinkError> {
        let raw = linux_events(event)?;
        if raw.is_empty() {
            return Ok(());
        }
        let events: Vec<InputEvent> = raw.iter().map(|e| InputEvent::new(e.kind, e.code, e.value)).collect();
        self.device.emit(&events).map_err(|error| SinkError(format!("uinput write failed: {error}")))
    }
}
