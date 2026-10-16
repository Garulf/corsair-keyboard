use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Client, ClientError};

pub const START_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launched {
    AlreadyRunning,
    Started,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("cannot find {}; keep k100 and k100d in the same folder", .0.display())]
    Missing(PathBuf),
    #[error("cannot start k100d: {0}")]
    Spawn(#[from] io::Error),
    #[error("k100d did not answer within {0:?}")]
    Timeout(Duration),
}

pub fn binary_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_owned() }
}

pub fn sibling_binary(base: &str) -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let folder = exe.parent().ok_or_else(|| io::Error::other("the running program has no folder"))?;
    Ok(folder.join(binary_name(base)))
}

pub fn spawn_detached(program: &Path, args: &[String]) -> io::Result<()> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    detach(&mut command);
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

pub fn ensure_running(
    start: impl FnOnce() -> Result<(), LaunchError>,
    mut reachable: impl FnMut() -> bool,
    timeout: Duration,
) -> Result<Launched, LaunchError> {
    if reachable() {
        return Ok(Launched::AlreadyRunning);
    }
    start()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if reachable() {
            return Ok(Launched::Started);
        }
        std::thread::sleep(PROBE_INTERVAL);
    }
    if reachable() { Ok(Launched::Started) } else { Err(LaunchError::Timeout(timeout)) }
}

pub fn daemon_reachable() -> bool {
    !matches!(Client::connect(), Err(ClientError::Io(_) | ClientError::Closed))
}

pub fn ensure_daemon(args: &[String]) -> Result<Launched, LaunchError> {
    let daemon = sibling_binary("k100d")?;
    ensure_running(
        || {
            if !daemon.exists() {
                return Err(LaunchError::Missing(daemon.clone()));
            }
            Ok(spawn_detached(&daemon, args)?)
        },
        daemon_reachable,
        START_TIMEOUT,
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn binaries_get_the_platform_suffix() {
        assert_eq!(binary_name("k100d"), if cfg!(windows) { "k100d.exe" } else { "k100d" });
        let sibling = sibling_binary("k100").unwrap();
        assert!(sibling.file_name().unwrap().to_string_lossy().starts_with("k100"));
        assert_eq!(sibling.parent(), std::env::current_exe().unwrap().parent());
    }

    #[test]
    fn a_running_daemon_is_never_started_twice() {
        let started = Cell::new(false);
        let outcome = ensure_running(
            || {
                started.set(true);
                Ok(())
            },
            || true,
            Duration::from_millis(50),
        )
        .unwrap();
        assert_eq!(outcome, Launched::AlreadyRunning);
        assert!(!started.get());
    }

    #[test]
    fn starts_once_then_waits_until_reachable() {
        let starts = Cell::new(0);
        let probes = Cell::new(0);
        let outcome = ensure_running(
            || {
                starts.set(starts.get() + 1);
                Ok(())
            },
            || {
                probes.set(probes.get() + 1);
                probes.get() > 3
            },
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(outcome, Launched::Started);
        assert_eq!(starts.get(), 1);
    }

    #[test]
    fn gives_up_after_the_timeout() {
        let error = ensure_running(|| Ok(()), || false, Duration::from_millis(250)).unwrap_err();
        assert!(matches!(error, LaunchError::Timeout(_)), "{error}");
    }

    #[test]
    fn a_failed_start_is_reported_without_waiting() {
        let started = Instant::now();
        let error =
            ensure_running(|| Err(LaunchError::Missing(PathBuf::from("k100d"))), || false, Duration::from_secs(5))
                .unwrap_err();
        assert!(matches!(error, LaunchError::Missing(_)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn spawning_detaches_and_reports_missing_programs() {
        spawn_detached(Path::new("/bin/true"), &[]).unwrap();
        assert!(spawn_detached(Path::new("/definitely/not/here"), &[]).is_err());
    }
}
