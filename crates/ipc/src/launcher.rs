use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
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
    #[error("k100d exited with {0}")]
    Exited(ExitStatus),
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

pub type ExitWatch = Receiver<ExitStatus>;

pub fn spawn_detached(program: &Path, args: &[String]) -> io::Result<ExitWatch> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    detach(&mut command);
    let mut child = command.spawn()?;
    let (exits, watch) = mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(status) = child.wait() {
            let _ = exits.send(status);
        }
    });
    Ok(watch)
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
    keep_std_handles_private();
}

#[cfg(windows)]
fn keep_std_handles_private() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    for handle in [io::stdin().as_raw_handle(), io::stdout().as_raw_handle(), io::stderr().as_raw_handle()] {
        if handle.is_null() {
            continue;
        }
        // SAFETY: handle is one of this process's standard handles; clearing its inherit flag only changes whether
        // children created with handle inheritance receive it, and failure (an invalid handle) is harmless.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
    }
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

pub fn ensure_running(
    start: impl FnOnce() -> Result<(), LaunchError>,
    reachable: impl FnMut() -> bool,
    timeout: Duration,
) -> Result<Launched, LaunchError> {
    ensure_running_watched(|| start().map(|()| None), reachable, timeout)
}

pub fn ensure_running_watched(
    start: impl FnOnce() -> Result<Option<ExitWatch>, LaunchError>,
    mut reachable: impl FnMut() -> bool,
    timeout: Duration,
) -> Result<Launched, LaunchError> {
    if reachable() {
        return Ok(Launched::AlreadyRunning);
    }
    let mut watch = start()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if reachable() {
            return Ok(Launched::Started);
        }
        match &watch {
            Some(exits) => match exits.recv_timeout(PROBE_INTERVAL) {
                Ok(status) if lost_the_lock_race(status) => watch = None,
                Ok(status) => {
                    return if reachable() { Ok(Launched::Started) } else { Err(LaunchError::Exited(status)) };
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            },
            None => std::thread::sleep(PROBE_INTERVAL),
        }
    }
    if reachable() { Ok(Launched::Started) } else { Err(LaunchError::Timeout(timeout)) }
}

fn lost_the_lock_race(status: ExitStatus) -> bool {
    status.code() == Some(i32::from(crate::ALREADY_RUNNING))
}

pub fn daemon_reachable() -> bool {
    !matches!(Client::connect(), Err(ClientError::Io(_) | ClientError::Closed))
}

pub fn ensure_daemon(args: &[String]) -> Result<Launched, LaunchError> {
    let daemon = sibling_binary("k100d")?;
    ensure_running_watched(
        || {
            if !daemon.exists() {
                return Err(LaunchError::Missing(daemon.clone()));
            }
            Ok(Some(spawn_detached(&daemon, args)?))
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

    #[cfg(windows)]
    #[test]
    fn spawning_stops_the_callers_std_handles_from_reaching_the_child() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation};

        let handle = io::stderr().as_raw_handle();
        // SAFETY: handle is this process's own stderr handle, which stays open for the whole test.
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            return;
        }
        keep_std_handles_private();
        let mut flags = 0u32;
        // SAFETY: handle is still open and flags is a valid out pointer.
        assert_ne!(unsafe { GetHandleInformation(handle, &raw mut flags) }, 0);
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
    }

    #[cfg(unix)]
    #[test]
    fn spawning_detaches_and_reports_missing_programs() {
        spawn_detached(Path::new("/bin/true"), &[]).unwrap();
        assert!(spawn_detached(Path::new("/definitely/not/here"), &[]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_dies_at_startup_is_reported_before_the_deadline() {
        let started = Instant::now();
        let error = ensure_running_watched(
            || Ok(Some(spawn_detached(Path::new("/bin/false"), &[])?)),
            || false,
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert!(matches!(error, LaunchError::Exited(status) if !status.success()), "{error}");
        assert!(error.to_string().contains("exited"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_exits_because_another_daemon_answers_counts_as_started() {
        use std::os::unix::process::ExitStatusExt;

        let probes = Cell::new(0);
        let outcome = ensure_running_watched(
            || {
                let (exits, watch) = mpsc::channel();
                exits.send(ExitStatus::from_raw(1 << 8)).unwrap();
                Ok(Some(watch))
            },
            || {
                probes.set(probes.get() + 1);
                probes.get() > 1
            },
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(outcome, Launched::Started);
    }

    #[cfg(unix)]
    fn already_running_watch() -> Result<Option<ExitWatch>, LaunchError> {
        use std::os::unix::process::ExitStatusExt;

        let (exits, watch) = mpsc::channel();
        exits.send(ExitStatus::from_raw(i32::from(crate::ALREADY_RUNNING) << 8)).unwrap();
        Ok(Some(watch))
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_lost_the_lock_race_waits_for_the_winner_to_answer() {
        let probes = Cell::new(0);
        let outcome = ensure_running_watched(
            already_running_watch,
            || {
                probes.set(probes.get() + 1);
                probes.get() > 4
            },
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(outcome, Launched::Started);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_lost_the_lock_race_times_out_when_the_winner_never_answers() {
        let error = ensure_running_watched(already_running_watch, || false, Duration::from_millis(300)).unwrap_err();
        assert!(matches!(error, LaunchError::Timeout(_)), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn spawning_works_from_a_folder_with_a_space_in_its_name() {
        let folder = tempfile::Builder::new().prefix("k100 spaced ").tempdir().unwrap();
        let program = folder.path().join("true");
        std::os::unix::fs::symlink("/bin/true", &program).unwrap();
        let exit = spawn_detached(&program, &[]).unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(exit.success());
    }
}
