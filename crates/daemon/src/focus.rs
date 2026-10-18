use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use ipc::Profile;

use crate::runtime::Command;

pub const FOCUS_POLL: Duration = Duration::from_millis(250);

pub fn matches(app: &str, exe_path: &str) -> bool {
    let app = app.trim();
    if app.is_empty() {
        return false;
    }
    if app.contains(['/', '\\']) {
        return exe_path.eq_ignore_ascii_case(app);
    }
    let file = exe_path.rsplit(['/', '\\']).next().unwrap_or(exe_path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    file.eq_ignore_ascii_case(app) || stem.eq_ignore_ascii_case(app)
}

pub fn profile_for<'a>(profiles: impl IntoIterator<Item = &'a Profile>, exe_path: &str) -> Option<String> {
    profiles
        .into_iter()
        .find(|profile| profile.apps.iter().any(|app| matches(app, exe_path)))
        .map(|profile| profile.name.clone())
}

pub trait ForegroundSource: Send {
    fn foreground_exe(&mut self) -> Option<String>;
}

pub fn spawn_watcher(mut source: Box<dyn ForegroundSource>, sender: Sender<Command>) {
    thread::spawn(move || {
        let mut last = None;
        loop {
            if let Some(current) = source.foreground_exe().filter(|current| last.as_ref() != Some(current)) {
                if sender.send(Command::Focus(Some(current.clone()))).is_err() {
                    return;
                }
                last = Some(current);
            }
            thread::sleep(FOCUS_POLL);
        }
    });
}

#[cfg(windows)]
pub fn platform_source() -> Option<Box<dyn ForegroundSource>> {
    Some(Box::new(windows::WindowsForeground))
}

#[cfg(not(windows))]
pub fn platform_source() -> Option<Box<dyn ForegroundSource>> {
    None
}

#[cfg(windows)]
pub(crate) use windows::image_path_of;

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    use super::ForegroundSource;

    pub struct WindowsForeground;

    impl ForegroundSource for WindowsForeground {
        fn foreground_exe(&mut self) -> Option<String> {
            // SAFETY: plain Win32 queries on a window handle that is only read, never owned or closed.
            let pid = unsafe {
                let window = GetForegroundWindow();
                if window.is_null() {
                    return None;
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(window, &mut pid);
                pid
            };
            if pid == 0 { None } else { image_path_of(pid) }
        }
    }

    pub(crate) fn image_path_of(pid: u32) -> Option<String> {
        // SAFETY: plain Win32 queries on handles owned by this function; the process handle is closed before returning.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut buffer = [0u16; 1024];
            let mut length = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length);
            CloseHandle(process);
            (ok != 0).then(|| String::from_utf16_lossy(&buffer[..length as usize]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_profile;

    #[test]
    fn app_entries_match_file_names_or_whole_paths() {
        assert!(matches("Game.exe", r"C:\Games\game.exe"));
        assert!(matches("game", r"C:\Games\game.exe"));
        assert!(matches("firefox", "/usr/lib/firefox/firefox"));
        assert!(matches(r"c:\games\game.exe", r"C:\Games\game.exe"));
        assert!(!matches(r"D:\game.exe", r"C:\Games\game.exe"));
        assert!(!matches("gam", r"C:\Games\game.exe"));
        assert!(!matches("", r"C:\Games\game.exe"));
    }

    struct Readings(Vec<Option<String>>);

    impl ForegroundSource for Readings {
        fn foreground_exe(&mut self) -> Option<String> {
            if self.0.len() > 1 { self.0.remove(0) } else { self.0[0].clone() }
        }
    }

    #[test]
    fn the_watcher_skips_transient_empty_readings() {
        let (a, b) = (Some("a.exe".to_owned()), Some("b.exe".to_owned()));
        let (sender, receiver) = std::sync::mpsc::channel();
        spawn_watcher(Box::new(Readings(vec![a.clone(), None, a.clone(), b.clone()])), sender);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut sent = Vec::new();
        while sent.len() < 2 {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match receiver.recv_timeout(left) {
                Ok(Command::Focus(exe)) => sent.push(exe),
                Ok(_) => {}
                Err(error) => panic!("watcher stopped sending: {error}, got {sent:?}"),
            }
        }
        assert_eq!(sent, [a, b]);
    }

    #[test]
    fn the_first_matching_profile_wins() {
        let mut b = Profile { name: "b".into(), apps: vec!["game.exe".into()], ..default_profile() };
        let a = Profile { name: "a".into(), apps: vec!["GAME".into()], ..default_profile() };
        assert_eq!(profile_for([&a, &b], r"C:\game.exe").as_deref(), Some("a"));
        b.apps.clear();
        assert_eq!(profile_for([&b], r"C:\game.exe"), None);
    }
}
