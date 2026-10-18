#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Handshake = 0,
    GetDevices = 1,
    GetDeviceInfo = 2,
    GetPropertyInfo = 3,
    ReadProperty = 4,
    WriteProperty = 5,
    LuidForKeyName = 6,
    GetLedPositions = 7,
    GetLedColors = 8,
    SetLayerPriority = 9,
    SetLedColors = 10,
    FlushLedColors = 11,
    RequestControl = 12,
    ReleaseControl = 13,
    Subscribe = 14,
    Unsubscribe = 15,
    ConfigureKeyEvent = 16,
    Event = 17,
}

impl Method {
    pub fn id(self) -> i64 {
        self as i64
    }

    pub fn from_id(id: i64) -> Option<Method> {
        Some(match id {
            0 => Method::Handshake,
            1 => Method::GetDevices,
            2 => Method::GetDeviceInfo,
            3 => Method::GetPropertyInfo,
            4 => Method::ReadProperty,
            5 => Method::WriteProperty,
            6 => Method::LuidForKeyName,
            7 => Method::GetLedPositions,
            8 => Method::GetLedColors,
            9 => Method::SetLayerPriority,
            10 => Method::SetLedColors,
            11 => Method::FlushLedColors,
            12 => Method::RequestControl,
            13 => Method::ReleaseControl,
            14 => Method::Subscribe,
            15 => Method::Unsubscribe,
            16 => Method::ConfigureKeyEvent,
            17 => Method::Event,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Success = 0,
    NotConnected = 1,
    NoControl = 2,
    IncompatibleProtocol = 3,
    InvalidArguments = 4,
    InvalidOperation = 5,
    DeviceNotFound = 6,
    NotAllowed = 7,
}

impl ErrorCode {
    pub fn code(self) -> i64 {
        self as i64
    }
}

pub const STATE_REFUSED: i64 = 4;
pub const STATE_CONNECTED: i64 = 6;

const LIGHTING_BIT: u8 = 1;
const KEYS_BIT: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessLevel {
    Shared = 0,
    ExclusiveLighting = 1,
    ExclusiveKeys = 2,
    Exclusive = 3,
}

impl AccessLevel {
    pub fn from_id(id: i64) -> Option<Self> {
        Some(match id {
            0 => AccessLevel::Shared,
            1 => AccessLevel::ExclusiveLighting,
            2 => AccessLevel::ExclusiveKeys,
            3 => AccessLevel::Exclusive,
            _ => return None,
        })
    }

    pub fn from_bits(bits: u8) -> Self {
        match bits & (LIGHTING_BIT | KEYS_BIT) {
            LIGHTING_BIT => AccessLevel::ExclusiveLighting,
            KEYS_BIT => AccessLevel::ExclusiveKeys,
            0 => AccessLevel::Shared,
            _ => AccessLevel::Exclusive,
        }
    }

    pub fn bits(self) -> u8 {
        self as u8
    }

    pub fn lighting(self) -> bool {
        self.bits() & LIGHTING_BIT != 0
    }

    pub fn keys(self) -> bool {
        self.bits() & KEYS_BIT != 0
    }
}

pub mod device_type {
    pub const KEYBOARD: u32 = 0x0001;
    pub const ALL: u32 = 0xFFFF_FFFF;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropertyId {
    PropertyArray = 1,
    MicEnabled = 2,
    SurroundSoundEnabled = 3,
    SidetoneEnabled = 4,
    EqualizerPreset = 5,
    PhysicalLayout = 6,
    LogicalLayout = 7,
    MacroKeyArray = 8,
    BatteryLevel = 9,
    ChannelLedCount = 10,
    ChannelDeviceCount = 11,
    ChannelDeviceLedCountArray = 12,
    ChannelDeviceTypeArray = 13,
}

impl PropertyId {
    pub fn id(self) -> i64 {
        self as i64
    }

    pub fn from_id(id: i64) -> Option<Self> {
        Some(match id {
            1 => PropertyId::PropertyArray,
            2 => PropertyId::MicEnabled,
            3 => PropertyId::SurroundSoundEnabled,
            4 => PropertyId::SidetoneEnabled,
            5 => PropertyId::EqualizerPreset,
            6 => PropertyId::PhysicalLayout,
            7 => PropertyId::LogicalLayout,
            8 => PropertyId::MacroKeyArray,
            9 => PropertyId::BatteryLevel,
            10 => PropertyId::ChannelLedCount,
            11 => PropertyId::ChannelDeviceCount,
            12 => PropertyId::ChannelDeviceLedCountArray,
            13 => PropertyId::ChannelDeviceTypeArray,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    Boolean = 0,
    Int32 = 1,
    Float64 = 2,
    String = 3,
    BooleanArray = 16,
    Int32Array = 17,
    Float64Array = 18,
    StringArray = 19,
}

impl DataType {
    pub fn id(self) -> i64 {
        self as i64
    }
}

pub mod property_flag {
    pub const CAN_READ: u32 = 0x01;
    pub const CAN_WRITE: u32 = 0x02;
    pub const INDEXED: u32 = 0x04;
}

pub mod physical_layout {
    pub const US: i32 = 1;
    pub const UK: i32 = 2;
    pub const JP: i32 = 3;
    pub const KR: i32 = 4;
    pub const BR: i32 = 5;
}

pub mod logical_layout {
    pub const US_INT: i32 = 1;
    pub const NA: i32 = 2;
    pub const EU: i32 = 3;
    pub const UK: i32 = 4;
    pub const BR: i32 = 6;
    pub const JP: i32 = 15;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventId {
    DeviceConnectionStatus = 1,
    Key = 2,
}

impl EventId {
    pub fn id(self) -> i64 {
        self as i64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LedGroup {
    Keyboard = 0,
    GKeys = 1,
    Edge = 2,
    Oem = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Luid(pub u32);

impl Luid {
    pub fn new(group: LedGroup, index: u16) -> Self {
        Luid((group as u32) << 16 | index as u32)
    }

    pub fn group(self) -> u16 {
        (self.0 >> 16) as u16
    }

    pub fn index(self) -> u16 {
        self.0 as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacroKey(pub u8);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luids_pack_group_and_index() {
        assert_eq!(Luid::new(LedGroup::GKeys, 1), Luid(0x0001_0001));
        assert_eq!((Luid(0x0002_002D).group(), Luid(0x0002_002D).index()), (2, 45));
    }

    #[test]
    fn method_ids_round_trip() {
        for id in 0..=17 {
            assert_eq!(Method::from_id(id).unwrap().id(), id);
        }
        assert_eq!(Method::from_id(18), None);
        assert_eq!(Method::from_id(-1), None);
        assert_eq!(AccessLevel::from_id(3).map(|l| (l.lighting(), l.keys())), Some((true, true)));
    }

    #[test]
    fn access_levels_are_capability_flags() {
        let bits = |id| AccessLevel::from_id(id).map(AccessLevel::bits);
        assert_eq!([bits(0), bits(1), bits(2), bits(3), bits(4)], [Some(0), Some(1), Some(2), Some(3), None]);
        assert_eq!(AccessLevel::from_id(1).map(|l| (l.lighting(), l.keys())), Some((true, false)));
        assert_eq!(AccessLevel::from_id(2).map(|l| (l.lighting(), l.keys())), Some((false, true)));
        assert_eq!((0..=3).map(AccessLevel::from_bits).map(AccessLevel::bits).collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(AccessLevel::from_bits(0xFE), AccessLevel::ExclusiveKeys);
    }

    #[test]
    fn error_codes_and_states_carry_the_documented_numbers() {
        assert_eq!(ErrorCode::NotConnected.code(), 1);
        assert_eq!(ErrorCode::NotAllowed.code(), 7);
        assert_eq!((STATE_REFUSED, STATE_CONNECTED), (4, 6));
    }

    #[test]
    fn property_ids_parse_the_documented_range() {
        assert_eq!(PropertyId::from_id(8), Some(PropertyId::MacroKeyArray));
        assert_eq!(PropertyId::from_id(0), None);
        assert_eq!(PropertyId::from_id(14), None);
    }
}
