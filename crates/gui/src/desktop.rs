const WSF_VISIBLE: u32 = 0x0001;

pub fn require_visible_station(flags: u32) -> Result<(), String> {
    if flags & WSF_VISIBLE == 0 {
        return Err("no interactive desktop: k100 gui needs a signed-in Windows session".into());
    }
    Ok(())
}

#[cfg(windows)]
pub fn require_interactive_desktop() -> Result<(), String> {
    station_flags().map_or(Ok(()), require_visible_station)
}

#[cfg(windows)]
fn station_flags() -> Option<u32> {
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS,
    };

    // SAFETY: GetProcessWindowStation takes no arguments and returns a borrowed handle that must not be closed.
    let station = unsafe { GetProcessWindowStation() };
    if station.is_null() {
        return None;
    }
    let mut flags = USEROBJECTFLAGS { fInherit: 0, fReserved: 0, dwFlags: 0 };
    let size = u32::try_from(size_of::<USEROBJECTFLAGS>()).ok()?;
    let mut needed = 0u32;
    // SAFETY: station is the live process window station, and the buffer is a USEROBJECTFLAGS of the size passed in.
    let read = unsafe { GetUserObjectInformationW(station, UOI_FLAGS, (&raw mut flags).cast(), size, &raw mut needed) };
    (read != 0).then_some(flags.dwFlags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_visible_window_station_counts_as_an_interactive_desktop() {
        assert!(require_visible_station(WSF_VISIBLE).is_ok());
        assert!(require_visible_station(WSF_VISIBLE | 0x0010).is_ok());
        assert!(require_visible_station(0).unwrap_err().contains("no interactive desktop"));
        assert!(require_visible_station(0x0010).is_err());
    }
}
