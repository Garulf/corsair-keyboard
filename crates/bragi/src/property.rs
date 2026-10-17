#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PropertyId(pub u16);

impl PropertyId {
    pub const POLLING_RATE: Self = Self(0x01);
    pub const BRIGHTNESS: Self = Self(0x02);
    pub const OPERATING_MODE: Self = Self(0x03);
    pub const PRESS_DEBOUNCE_ENABLED: Self = Self(0x09);
    pub const PRESS_DEBOUNCE_TIMEOUT: Self = Self(0x0A);
    pub const USB_PID: Self = Self(0x12);
    pub const FIRMWARE_VERSION: Self = Self(0x13);
    pub const RELEASE_DEBOUNCE_TIMEOUT: Self = Self(0x38);
    pub const RELEASE_DEBOUNCE_ENABLED: Self = Self(0x39);
    pub const PHYSICAL_KEYBOARD_LAYOUT: Self = Self(0x41);
    pub const FREE_STORAGE_SIZE: Self = Self(0x3E);
    pub const WIN_LOCK_ENABLED: Self = Self(0x45);
    pub const WIN_LOCK_DISABLED_SHORTCUTS: Self = Self(0x4A);
    pub const DIAL_RING_ROTATION: Self = Self(0x60);
    pub const DIAL_ZONE_BRIGHTNESS: Self = Self(0x6D);
    pub const ACTIVE_HARDWARE_PROFILE_INDEX: Self = Self(0x6E);
    pub const KEY_PRESS_NOTIFICATIONS_REPORTING: Self = Self(0x92);
    pub const MAXIMUM_POLLING_RATE: Self = Self(0x96);
    pub const BRAGI_PROTOCOL_VERSION: Self = Self(0x105);
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    Hardware = 1,
    Software = 2,
}

impl OperatingMode {
    pub fn raw(self) -> u32 {
        self as u32
    }

    pub fn from_raw(raw: u32) -> Option<Self> {
        [Self::Hardware, Self::Software].into_iter().find(|mode| mode.raw() == raw)
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PollingRate {
    Hz125 = 1,
    Hz250 = 2,
    Hz500 = 3,
    Hz1000 = 4,
    Hz2000 = 5,
    Hz4000 = 6,
    Hz8000 = 7,
}

impl PollingRate {
    pub const ALL: [Self; 7] =
        [Self::Hz125, Self::Hz250, Self::Hz500, Self::Hz1000, Self::Hz2000, Self::Hz4000, Self::Hz8000];

    pub fn raw(self) -> u32 {
        self as u32
    }

    pub fn hz(self) -> u32 {
        125 << (self.raw() - 1)
    }

    pub fn from_raw(raw: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|rate| rate.raw() == raw)
    }

    pub fn from_hz(hz: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|rate| rate.hz() == hz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_rate_round_trips() {
        assert_eq!(PollingRate::Hz8000.raw(), 7);
        assert_eq!(PollingRate::from_raw(4), Some(PollingRate::Hz1000));
        assert_eq!(PollingRate::from_raw(0), None);
        assert_eq!(PollingRate::from_hz(250), Some(PollingRate::Hz250));
        assert_eq!(PollingRate::Hz2000.hz(), 2000);
        assert_eq!(PollingRate::from_hz(300), None);
    }

    #[test]
    fn operating_mode_values() {
        assert_eq!(OperatingMode::Software.raw(), 2);
        assert_eq!(OperatingMode::from_raw(1), Some(OperatingMode::Hardware));
        assert_eq!(OperatingMode::from_raw(3), None);
    }
}
