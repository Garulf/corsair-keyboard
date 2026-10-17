mod client;
mod frame;
mod framing;
pub mod launcher;
mod lock;
mod messages;
mod settings;

pub use client::{Client, ClientError, lock_path, socket_name, socket_path};
pub use frame::{decode_frame, encode_frame};
pub use framing::{MAX_LINE, read_message, write_message};
pub use lock::{ALREADY_RUNNING, InstanceLock, startup_exit_code};
pub use messages::{
    AudioState, DaemonState, DeviceState, Event, OnboardRequest, OnboardSlot, PROTOCOL_VERSION, Profile, Reply, Request,
};
pub use settings::{Debounce, DialRing, MAX_DEBOUNCE_TIMEOUT, Settings, SettingsPatch, WinLockShortcut};
