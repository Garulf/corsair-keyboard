use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::runtime::Command;

pub const CONSOLE_CLOSE_GRACE: Duration = Duration::from_secs(4);

#[derive(Clone, Default)]
pub struct ReleaseSignal(Arc<(Mutex<bool>, Condvar)>);

impl ReleaseSignal {
    pub fn notify(&self) {
        let (released, condvar) = &*self.0;
        *released.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        condvar.notify_all();
    }

    pub fn wait(&self, timeout: Duration) -> bool {
        let (released, condvar) = &*self.0;
        let guard = released.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (guard, _) = condvar
            .wait_timeout_while(guard, timeout, |released| !*released)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard
    }
}

#[cfg(not(windows))]
pub fn install(commands: Sender<Command>, _released: ReleaseSignal) -> Result<(), ctrlc::Error> {
    let commands = Mutex::new(commands);
    ctrlc::set_handler(move || {
        if let Ok(commands) = commands.lock() {
            let _ = commands.send(Command::Shutdown);
        }
    })
}

#[cfg(windows)]
pub fn install(commands: Sender<Command>, released: ReleaseSignal) -> std::io::Result<()> {
    windows_console::install(commands, released)
}

#[cfg(windows)]
mod windows_console {
    use std::sync::mpsc::Sender;
    use std::sync::{Mutex, OnceLock};

    use windows_sys::Win32::Foundation::{FALSE, TRUE};
    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::core::BOOL;

    use super::{CONSOLE_CLOSE_GRACE, ReleaseSignal};
    use crate::runtime::Command;

    static STATE: OnceLock<(Mutex<Sender<Command>>, ReleaseSignal)> = OnceLock::new();

    unsafe extern "system" fn handler(event: u32) -> BOOL {
        let Some((commands, released)) = STATE.get() else { return FALSE };
        if let Ok(commands) = commands.lock() {
            let _ = commands.send(Command::Shutdown);
        }
        if matches!(event, CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT) {
            released.wait(CONSOLE_CLOSE_GRACE);
        }
        TRUE
    }

    pub fn install(commands: Sender<Command>, released: ReleaseSignal) -> std::io::Result<()> {
        STATE
            .set((Mutex::new(commands), released))
            .map_err(|_| std::io::Error::other("console handler already installed"))?;
        if unsafe { SetConsoleCtrlHandler(Some(handler), TRUE) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn wait_returns_once_released() {
        let signal = ReleaseSignal::default();
        let notifier = signal.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            notifier.notify();
        });
        assert!(signal.wait(Duration::from_secs(5)));
    }

    #[test]
    fn wait_gives_up_after_the_timeout() {
        let started = Instant::now();
        assert!(!ReleaseSignal::default().wait(Duration::from_millis(100)));
        assert!(started.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn notify_before_wait_is_remembered() {
        let signal = ReleaseSignal::default();
        signal.notify();
        assert!(signal.wait(Duration::ZERO));
    }
}
