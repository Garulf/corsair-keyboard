use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ipc::{DaemonState, DeviceState, Event, Reply};
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::runtime::{Command, SUBSCRIBER_QUEUE};
use crate::tray_menu::{
    self, ICON_SIZE, MenuEntry, TrayCommand, dispatch, expected_checks, fetch_state, open_gui, parse_command,
};

const LOOP_TICK: Duration = Duration::from_millis(50);
const REFRESH_EVERY: Duration = Duration::from_secs(5);

pub struct TrayHandle {
    stop: Sender<()>,
    thread: JoinHandle<()>,
}

impl TrayHandle {
    pub fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.thread.join();
    }
}

pub fn spawn(commands: Sender<Command>) -> io::Result<TrayHandle> {
    let (stop, stopped) = mpsc::channel();
    let thread = thread::Builder::new().name("k100d-tray".into()).spawn(move || run(&commands, &stopped))?;
    Ok(TrayHandle { stop, thread })
}

#[derive(PartialEq)]
struct Shown {
    entries: Vec<MenuEntry>,
    tooltip: String,
    device: DeviceState,
}

fn shown_for(state: &DaemonState) -> Shown {
    Shown { entries: tray_menu::menu(state), tooltip: tray_menu::tooltip(state), device: state.device.clone() }
}

fn run(commands: &Sender<Command>, stopped: &Receiver<()>) {
    if let Err(message) = init_platform() {
        eprintln!("k100d: tray unavailable: {message}");
        return;
    }
    let state = fetch_state(commands).unwrap_or_default();
    let (tray, mut checks, mut shown) = match create(&state) {
        Ok(created) => created,
        Err(message) => {
            eprintln!("k100d: tray unavailable: {message}");
            return;
        }
    };
    let Some(mut events) = subscribe(commands) else { return };
    let mut next_refresh = Instant::now() + REFRESH_EVERY;
    loop {
        pump();
        let mut due = Instant::now() >= next_refresh;
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(command) = parse_command(&event.id().0) {
                dispatch(command, commands, open_gui);
                due = true;
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                dispatch(TrayCommand::Open, commands, open_gui);
            }
        }
        match events.recv_timeout(LOOP_TICK) {
            Ok(Reply::Event { event }) => {
                due |= matches!(event, Event::ProfileChanged { .. } | Event::DeviceState { .. });
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                thread::sleep(LOOP_TICK);
                let Some(fresh) = subscribe(commands) else { return };
                events = fresh;
                due = true;
            }
        }
        if due {
            next_refresh = Instant::now() + REFRESH_EVERY;
            refresh(&tray, commands, &mut checks, &mut shown);
        }
        if !matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
            return;
        }
    }
}

fn subscribe(commands: &Sender<Command>) -> Option<Receiver<Reply>> {
    let (events, receiver) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
    commands.send(Command::Subscribe { events, frames: false }).ok()?;
    Some(receiver)
}

fn create(state: &DaemonState) -> Result<(TrayIcon, Vec<CheckMenuItem>, Shown), String> {
    let shown = shown_for(state);
    let (menu, checks) = build_menu(&shown.entries)?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(&shown.tooltip)
        .with_icon(icon(&shown.device)?)
        .build()
        .map_err(|failure| failure.to_string())?;
    Ok((tray, checks, shown))
}

fn refresh(tray: &TrayIcon, commands: &Sender<Command>, checks: &mut Vec<CheckMenuItem>, shown: &mut Shown) {
    let Some(state) = fetch_state(commands) else { return };
    let next = shown_for(&state);
    if next.entries != shown.entries
        && let Ok((menu, rebuilt)) = build_menu(&next.entries)
    {
        tray.set_menu(Some(Box::new(menu)));
        *checks = rebuilt;
    }
    resync_checks(checks, &next.entries);
    if next.tooltip != shown.tooltip {
        let _ = tray.set_tooltip(Some(&next.tooltip));
    }
    if next.device != shown.device
        && let Ok(icon) = icon(&next.device)
    {
        let _ = tray.set_icon(Some(icon));
    }
    *shown = next;
}

fn resync_checks(checks: &[CheckMenuItem], entries: &[MenuEntry]) {
    for (id, checked) in expected_checks(entries) {
        if let Some(item) = checks.iter().find(|item| item.id().0 == id) {
            item.set_checked(checked);
        }
    }
}

fn icon(device: &DeviceState) -> Result<Icon, String> {
    Icon::from_rgba(tray_menu::icon_rgba(device), ICON_SIZE, ICON_SIZE).map_err(|failure| failure.to_string())
}

fn build_menu(entries: &[MenuEntry]) -> Result<(Menu, Vec<CheckMenuItem>), String> {
    let menu = Menu::new();
    let mut checks = Vec::new();
    for entry in entries {
        menu.append(&*to_item(entry, &mut checks)?).map_err(|failure| failure.to_string())?;
    }
    Ok((menu, checks))
}

fn to_item(entry: &MenuEntry, checks: &mut Vec<CheckMenuItem>) -> Result<Box<dyn IsMenuItem>, String> {
    let item: Box<dyn IsMenuItem> = match entry {
        MenuEntry::Item { id, text, enabled } => Box::new(MenuItem::with_id(id.as_str(), text, *enabled, None)),
        MenuEntry::Check { id, text, checked } => {
            let item = CheckMenuItem::with_id(id.as_str(), text, true, *checked, None);
            checks.push(item.clone());
            Box::new(item)
        }
        MenuEntry::Separator => Box::new(PredefinedMenuItem::separator()),
        MenuEntry::Submenu { text, entries } => {
            let submenu = Submenu::new(text, true);
            for child in entries {
                submenu.append(&*to_item(child, checks)?).map_err(|failure| failure.to_string())?;
            }
            Box::new(submenu)
        }
    };
    Ok(item)
}

#[cfg(windows)]
fn init_platform() -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn pump() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    // SAFETY: MSG is a plain C struct for which all-zero bytes are a valid value.
    let mut message: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: `message` is a valid writable MSG and a null window handle polls this thread's queue.
    while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        // SAFETY: `message` was filled in by PeekMessageW on this thread.
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(not(windows))]
fn init_platform() -> Result<(), String> {
    gtk::init().map_err(|failure| failure.to_string())
}

#[cfg(not(windows))]
fn pump() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}
