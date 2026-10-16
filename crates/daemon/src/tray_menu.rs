use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use ipc::{DaemonState, DeviceState, Reply, Request};

use crate::runtime::Command;

pub const ICON_SIZE: u32 = 32;
const PROFILE_PREFIX: &str = "profile:";
const MAX_TOOLTIP: usize = 120;
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item { id: String, text: String, enabled: bool },
    Check { id: String, text: String, checked: bool },
    Separator,
    Submenu { text: String, entries: Vec<MenuEntry> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    SelectProfile(String),
    Release,
    Reacquire,
    Quit,
}

pub fn headline(device: &DeviceState) -> String {
    match device {
        DeviceState::Connected { model, .. } => format!("{model} connected"),
        DeviceState::Disconnected => "No keyboard found".to_owned(),
        DeviceState::Blocked { processes } => format!("Blocked by {}", processes.join(", ")),
        DeviceState::Released => "Released to hardware mode".to_owned(),
        DeviceState::Paused { reason } => format!("Paused: {reason}"),
    }
}

pub fn tooltip(state: &DaemonState) -> String {
    let text = format!("K100: {} | profile {}", headline(&state.device), state.active_profile);
    text.chars().take(MAX_TOOLTIP).collect()
}

pub fn menu(state: &DaemonState) -> Vec<MenuEntry> {
    let mut profiles: Vec<MenuEntry> = state
        .profiles
        .iter()
        .map(|name| MenuEntry::Check {
            id: format!("{PROFILE_PREFIX}{name}"),
            text: name.clone(),
            checked: *name == state.active_profile,
        })
        .collect();
    if profiles.is_empty() {
        profiles.push(MenuEntry::Item { id: "no-profiles".into(), text: "No profiles".into(), enabled: false });
    }
    let hardware = if state.device == DeviceState::Released {
        MenuEntry::Item { id: "reacquire".into(), text: "Take the keyboard back".into(), enabled: true }
    } else {
        MenuEntry::Item { id: "release".into(), text: "Release to hardware mode".into(), enabled: true }
    };
    vec![
        MenuEntry::Item { id: "status".into(), text: headline(&state.device), enabled: false },
        MenuEntry::Separator,
        MenuEntry::Submenu { text: "Profiles".into(), entries: profiles },
        MenuEntry::Separator,
        MenuEntry::Item { id: "open".into(), text: "Open K100".into(), enabled: true },
        hardware,
        MenuEntry::Separator,
        MenuEntry::Item { id: "quit".into(), text: "Quit".into(), enabled: true },
    ]
}

pub fn expected_checks(entries: &[MenuEntry]) -> Vec<(String, bool)> {
    let mut checks = Vec::new();
    for entry in entries {
        match entry {
            MenuEntry::Check { id, checked, .. } => checks.push((id.clone(), *checked)),
            MenuEntry::Submenu { entries, .. } => checks.extend(expected_checks(entries)),
            MenuEntry::Item { .. } | MenuEntry::Separator => {}
        }
    }
    checks
}

pub fn parse_command(id: &str) -> Option<TrayCommand> {
    match id {
        "open" => Some(TrayCommand::Open),
        "release" => Some(TrayCommand::Release),
        "reacquire" => Some(TrayCommand::Reacquire),
        "quit" => Some(TrayCommand::Quit),
        other => other.strip_prefix(PROFILE_PREFIX).map(|name| TrayCommand::SelectProfile(name.to_owned())),
    }
}

pub fn icon_rgba(device: &DeviceState) -> Vec<u8> {
    let [red, green, blue] = match device {
        DeviceState::Connected { .. } => [0x2E, 0xCC, 0x71],
        DeviceState::Disconnected => [0x95, 0xA5, 0xA6],
        DeviceState::Blocked { .. } => [0xF3, 0x9C, 0x12],
        DeviceState::Released => [0x34, 0x98, 0xDB],
        DeviceState::Paused { .. } => [0xE7, 0x4C, 0x3C],
    };
    let size = ICON_SIZE as f32;
    let radius = size / 2.0 - 1.5;
    let mut pixels = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let (dx, dy) = (x as f32 + 0.5 - size / 2.0, y as f32 + 0.5 - size / 2.0);
            let alpha = if (dx * dx + dy * dy).sqrt() <= radius { 255 } else { 0 };
            pixels.extend_from_slice(&[red, green, blue, alpha]);
        }
    }
    pixels
}

pub fn ask(commands: &Sender<Command>, request: Request) -> Option<Reply> {
    let (reply_tx, reply_rx) = mpsc::channel();
    commands.send(Command::Request(request, reply_tx)).ok()?;
    reply_rx.recv_timeout(REPLY_TIMEOUT).ok()
}

pub fn fetch_state(commands: &Sender<Command>) -> Option<DaemonState> {
    match ask(commands, Request::GetState)? {
        Reply::State { state } => Some(state),
        _ => None,
    }
}

pub fn open_gui() {
    let Ok(k100) = ipc::launcher::sibling_binary("k100") else { return };
    if let Err(failure) = ipc::launcher::spawn_detached(&k100, &["gui".to_owned()]) {
        eprintln!("k100d: cannot open the GUI: {failure}");
    }
}

pub fn dispatch(command: TrayCommand, commands: &Sender<Command>, open_gui: impl FnOnce()) {
    match command {
        TrayCommand::Open => open_gui(),
        TrayCommand::SelectProfile(name) => {
            ask(commands, Request::SetActiveProfile { name });
        }
        TrayCommand::Release => {
            ask(commands, Request::Release);
        }
        TrayCommand::Reacquire => {
            ask(commands, Request::Reacquire);
        }
        TrayCommand::Quit => {
            let _ = commands.send(Command::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use ipc::{Reply, Request};

    use super::*;

    fn state() -> DaemonState {
        DaemonState {
            device: DeviceState::Connected { model: "K100 RGB".into(), firmware: "1.15.61".into() },
            active_profile: "games".into(),
            profiles: vec!["default".into(), "games".into()],
            ..DaemonState::default()
        }
    }

    #[test]
    fn the_menu_lists_profiles_with_the_active_one_checked() {
        let entries = menu(&state());
        let MenuEntry::Submenu { text, entries: profiles } = &entries[2] else { panic!("{entries:?}") };
        assert_eq!(text, "Profiles");
        assert_eq!(
            profiles,
            &[
                MenuEntry::Check { id: "profile:default".into(), text: "default".into(), checked: false },
                MenuEntry::Check { id: "profile:games".into(), text: "games".into(), checked: true },
            ]
        );
        assert_eq!(
            entries[0],
            MenuEntry::Item { id: "status".into(), text: "K100 RGB connected".into(), enabled: false }
        );
        let last = entries.last().unwrap();
        assert_eq!(last, &MenuEntry::Item { id: "quit".into(), text: "Quit".into(), enabled: true });
    }

    #[test]
    fn the_release_entry_flips_while_released() {
        let ids = |state: &DaemonState| -> Vec<String> {
            menu(state)
                .into_iter()
                .filter_map(|entry| match entry {
                    MenuEntry::Item { id, .. } => Some(id),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(ids(&state()), ["status", "open", "release", "quit"]);
        let released = DaemonState { device: DeviceState::Released, ..state() };
        assert_eq!(ids(&released), ["status", "open", "reacquire", "quit"]);
    }

    #[test]
    fn an_empty_profile_list_still_has_a_submenu_entry() {
        let entries = menu(&DaemonState::default());
        let MenuEntry::Submenu { entries: profiles, .. } = &entries[2] else { panic!() };
        assert_eq!(profiles.len(), 1);
        assert!(matches!(&profiles[0], MenuEntry::Item { enabled: false, .. }));
    }

    #[test]
    fn expected_checks_flatten_nested_check_items() {
        assert_eq!(
            expected_checks(&menu(&state())),
            [("profile:default".to_owned(), false), ("profile:games".to_owned(), true)]
        );
        assert!(expected_checks(&menu(&DaemonState::default())).is_empty());
    }

    #[test]
    fn menu_ids_map_back_to_commands() {
        assert_eq!(parse_command("open"), Some(TrayCommand::Open));
        assert_eq!(parse_command("quit"), Some(TrayCommand::Quit));
        assert_eq!(parse_command("release"), Some(TrayCommand::Release));
        assert_eq!(parse_command("reacquire"), Some(TrayCommand::Reacquire));
        assert_eq!(parse_command("profile:a:b"), Some(TrayCommand::SelectProfile("a:b".into())));
        assert_eq!(parse_command("status"), None);
    }

    #[test]
    fn headlines_name_the_blocking_processes() {
        let blocked = DeviceState::Blocked { processes: vec!["iCUE.exe".into(), "iCUEService.exe".into()] };
        assert_eq!(headline(&blocked), "Blocked by iCUE.exe, iCUEService.exe");
        assert_eq!(headline(&DeviceState::Disconnected), "No keyboard found");
    }

    #[test]
    fn tooltips_stay_within_the_windows_limit() {
        let long = DaemonState { device: DeviceState::Paused { reason: "\u{e9}".repeat(300) }, ..state() };
        let text = tooltip(&long);
        assert!(text.chars().count() <= 120);
        assert!(tooltip(&state()).starts_with("K100: K100 RGB connected"));
    }

    #[test]
    fn the_icon_is_a_disc_in_the_state_colour() {
        let pixels = icon_rgba(&state().device);
        assert_eq!(pixels.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let at = |x: u32, y: u32| {
            let start = ((y * ICON_SIZE + x) * 4) as usize;
            pixels[start..start + 4].to_vec()
        };
        assert_eq!(at(16, 16), [0x2E, 0xCC, 0x71, 255]);
        assert_eq!(at(0, 0)[3], 0);
        assert_ne!(icon_rgba(&DeviceState::Released), pixels);
    }

    #[test]
    fn quit_goes_through_the_runtime_shutdown() {
        let (commands, received) = mpsc::channel();
        dispatch(TrayCommand::Quit, &commands, || panic!("quit must not open the GUI"));
        assert!(matches!(received.try_recv().unwrap(), Command::Shutdown));
    }

    #[test]
    fn open_runs_the_launcher_and_sends_nothing() {
        let (commands, received) = mpsc::channel();
        let opened = Cell::new(false);
        dispatch(TrayCommand::Open, &commands, || opened.set(true));
        assert!(opened.get());
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn choosing_a_profile_sends_a_set_active_request() {
        let (commands, received) = mpsc::channel();
        let runtime = thread::spawn(move || {
            let Command::Request(request, reply) = received.recv_timeout(Duration::from_secs(2)).unwrap() else {
                panic!("expected a request");
            };
            reply.send(Reply::Done).unwrap();
            request
        });
        dispatch(TrayCommand::SelectProfile("games".into()), &commands, || {});
        assert_eq!(runtime.join().unwrap(), Request::SetActiveProfile { name: "games".into() });
    }

    #[test]
    fn fetch_state_asks_the_runtime() {
        let (commands, received) = mpsc::channel();
        let runtime = thread::spawn(move || {
            let Command::Request(Request::GetState, reply) = received.recv_timeout(Duration::from_secs(2)).unwrap()
            else {
                panic!("expected get_state");
            };
            reply.send(Reply::State { state: state() }).unwrap();
        });
        assert_eq!(fetch_state(&commands).unwrap().active_profile, "games");
        runtime.join().unwrap();
    }
}
