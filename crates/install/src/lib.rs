mod autostart;

pub use autostart::{Autostart, ENTRY_NAME, Unavailable, XdgEntry, desktop_entry, exec_line, platform, run_command};

#[cfg(windows)]
pub use autostart::RunKey;

#[cfg(test)]
mod packaging;
