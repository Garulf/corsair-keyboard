#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    SetProperty = 0x01,
    GetProperty = 0x02,
    UnbindBuffers = 0x05,
    WriteBufferBegin = 0x06,
    WriteBuffer = 0x07,
    ReadBuffer = 0x08,
    DescribeBuffer = 0x09,
    CreateFile = 0x0B,
    RemoveFile = 0x0C,
    OpenFile = 0x0D,
    Ping = 0x12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u16);

impl FileId {
    pub const KEY_MAPPINGS: Self = Self(0x02);
    pub const REAL_TIME_LIGHTINGS_BUFFER: Self = Self(0x22);
    pub const INDICATION_REAL_TIME_LIGHTINGS_BUFFER: Self = Self(0x2E);
}
