//! Stream Deck only calls `CorsairConnect` while a process whose image file is named exactly
//! `iCUE.exe` is running, so k100d keeps a copy of itself under that name running while the
//! SDK server is enabled. The copy does nothing but wait for k100d to close its stdin.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const MODE_ARG: &str = "--icue-standin";
pub const EXE_NAME: &str = "iCUE.exe";
const POLL: Duration = Duration::from_secs(1);
const RETRY_DELAY: Duration = Duration::from_secs(10);

pub fn wait_for_parent(mut reader: impl Read) {
    let mut buffer = [0u8; 64];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

pub fn refresh_copy(source: &Path, target: &Path) -> io::Result<()> {
    let wanted = std::fs::read(source)?;
    if std::fs::read(target).is_ok_and(|current| current == wanted) {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, target).map(drop)
}

pub trait StandIn: Send {
    fn id(&self) -> u32;
    fn exited(&mut self) -> bool;
    fn kill(&mut self);
}

pub trait Launcher {
    fn launch(&mut self) -> io::Result<Box<dyn StandIn>>;
}

pub struct Supervisor<L> {
    launcher: L,
    child: Option<Box<dyn StandIn>>,
    pid: Arc<AtomicU32>,
    retry_at: Option<Instant>,
    reported: bool,
}

impl<L: Launcher> Supervisor<L> {
    pub fn new(launcher: L) -> Supervisor<L> {
        Supervisor { launcher, child: None, pid: Arc::new(AtomicU32::new(0)), retry_at: None, reported: false }
    }

    pub fn pid(&self) -> Arc<AtomicU32> {
        self.pid.clone()
    }

    pub fn step(&mut self, enabled: bool, now: Instant) {
        if self.child.as_mut().is_some_and(|child| child.exited()) {
            self.child = None;
            self.pid.store(0, Ordering::SeqCst);
            self.retry_at = Some(now + RETRY_DELAY);
        }
        if !enabled {
            self.kill();
            self.retry_at = None;
            return;
        }
        if self.child.is_some() || self.retry_at.is_some_and(|at| now < at) {
            return;
        }
        match self.launcher.launch() {
            Ok(child) => {
                self.pid.store(child.id(), Ordering::SeqCst);
                self.child = Some(child);
                self.retry_at = None;
                self.reported = false;
            }
            Err(error) => {
                if !self.reported {
                    eprintln!("k100d: could not start the iCUE.exe stand-in: {error}");
                    self.reported = true;
                }
                self.retry_at = Some(now + RETRY_DELAY);
            }
        }
    }

    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill();
        }
        self.pid.store(0, Ordering::SeqCst);
    }
}

pub struct SupervisorHandle {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
    pid: Arc<AtomicU32>,
}

impl SupervisorHandle {
    pub fn pid(&self) -> Arc<AtomicU32> {
        self.pid.clone()
    }

    pub fn stop(self) {
        drop(self.stop);
        let _ = self.thread.join();
    }
}

pub fn spawn<L: Launcher + Send + 'static>(launcher: L, enabled: Arc<AtomicBool>) -> SupervisorHandle {
    let mut supervisor = Supervisor::new(launcher);
    let pid = supervisor.pid();
    supervisor.step(enabled.load(Ordering::SeqCst), Instant::now());
    let (stop, stopped) = mpsc::channel::<()>();
    let thread = thread::spawn(move || {
        while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(POLL) {
            supervisor.step(enabled.load(Ordering::SeqCst), Instant::now());
        }
        supervisor.kill();
    });
    SupervisorHandle { stop, thread, pid }
}

pub struct ProcessLauncher {
    source: PathBuf,
    target: PathBuf,
}

impl ProcessLauncher {
    pub fn new(source: PathBuf, target: PathBuf) -> ProcessLauncher {
        ProcessLauncher { source, target }
    }
}

impl Launcher for ProcessLauncher {
    fn launch(&mut self) -> io::Result<Box<dyn StandIn>> {
        refresh_copy(&self.source, &self.target)?;
        let mut command = Command::new(&self.target);
        command.arg(MODE_ARG).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Ok(Box::new(ChildStandIn(command.spawn()?)))
    }
}

struct ChildStandIn(Child);

impl StandIn for ChildStandIn {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn exited(&mut self) -> bool {
        !matches!(self.0.try_wait(), Ok(None))
    }

    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Log {
        launches: u32,
        kills: u32,
        exited: bool,
        fail: bool,
    }

    struct FakeLauncher(Arc<Mutex<Log>>);
    struct FakeStandIn(Arc<Mutex<Log>>, u32);

    impl Launcher for FakeLauncher {
        fn launch(&mut self) -> io::Result<Box<dyn StandIn>> {
            let mut log = self.0.lock().unwrap();
            if log.fail {
                return Err(io::Error::other("quarantined"));
            }
            log.launches += 1;
            log.exited = false;
            Ok(Box::new(FakeStandIn(self.0.clone(), 100 + log.launches)))
        }
    }

    impl StandIn for FakeStandIn {
        fn id(&self) -> u32 {
            self.1
        }
        fn exited(&mut self) -> bool {
            self.0.lock().unwrap().exited
        }
        fn kill(&mut self) {
            self.0.lock().unwrap().kills += 1;
        }
    }

    fn rig() -> (Arc<Mutex<Log>>, Supervisor<FakeLauncher>) {
        let log = Arc::new(Mutex::new(Log::default()));
        let supervisor = Supervisor::new(FakeLauncher(log.clone()));
        (log, supervisor)
    }

    #[test]
    fn starts_once_while_enabled_and_stops_when_disabled() {
        let (log, mut supervisor) = rig();
        let pid = supervisor.pid();
        let now = Instant::now();
        supervisor.step(true, now);
        supervisor.step(true, now + POLL);
        assert_eq!(log.lock().unwrap().launches, 1);
        assert_eq!(pid.load(Ordering::SeqCst), 101);
        supervisor.step(false, now + POLL * 2);
        assert_eq!(log.lock().unwrap().kills, 1);
        assert_eq!(pid.load(Ordering::SeqCst), 0);
        supervisor.step(true, now + POLL * 3);
        assert_eq!(log.lock().unwrap().launches, 2);
    }

    #[test]
    fn a_disabled_supervisor_starts_nothing() {
        let (log, mut supervisor) = rig();
        supervisor.step(false, Instant::now());
        assert_eq!(log.lock().unwrap().launches, 0);
    }

    #[test]
    fn an_exited_standin_is_restarted_after_the_delay() {
        let (log, mut supervisor) = rig();
        let pid = supervisor.pid();
        let now = Instant::now();
        supervisor.step(true, now);
        log.lock().unwrap().exited = true;
        supervisor.step(true, now + POLL);
        assert_eq!(pid.load(Ordering::SeqCst), 0);
        assert_eq!(log.lock().unwrap().launches, 1);
        supervisor.step(true, now + POLL + RETRY_DELAY);
        assert_eq!(log.lock().unwrap().launches, 2);
        assert_eq!(pid.load(Ordering::SeqCst), 102);
    }

    #[test]
    fn a_failed_start_waits_before_retrying() {
        let (log, mut supervisor) = rig();
        let now = Instant::now();
        log.lock().unwrap().fail = true;
        supervisor.step(true, now);
        log.lock().unwrap().fail = false;
        supervisor.step(true, now + POLL);
        assert_eq!(log.lock().unwrap().launches, 0);
        supervisor.step(true, now + RETRY_DELAY);
        assert_eq!(log.lock().unwrap().launches, 1);
    }

    #[test]
    fn spawn_publishes_the_pid_before_returning_and_stop_kills_the_standin() {
        let log = Arc::new(Mutex::new(Log::default()));
        let handle = spawn(FakeLauncher(log.clone()), Arc::new(AtomicBool::new(true)));
        assert_eq!(handle.pid().load(Ordering::SeqCst), 101);
        let pid = handle.pid();
        handle.stop();
        assert_eq!(log.lock().unwrap().kills, 1);
        assert_eq!(pid.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_standin_returns_when_its_stdin_closes() {
        wait_for_parent(io::Cursor::new(b"anything".to_vec()));
    }

    #[test]
    fn refresh_copy_writes_missing_or_changed_copies_only() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("k100d");
        let target = temp.path().join("icue-standin").join(EXE_NAME);
        std::fs::write(&source, b"build 1").unwrap();
        refresh_copy(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"build 1");

        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&target, permissions).unwrap();
        refresh_copy(&source, &target).unwrap();

        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&target, permissions).unwrap();
        std::fs::write(&source, b"build 2").unwrap();
        refresh_copy(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"build 2");
    }

    #[cfg(unix)]
    #[test]
    fn the_process_launcher_starts_the_copy_and_kills_it() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("k100d");
        std::fs::write(&source, "#!/bin/sh\n[ \"$1\" = --icue-standin ] && exec cat\n").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = temp.path().join("icue-standin").join(EXE_NAME);
        let mut launcher = ProcessLauncher::new(source, target);
        let mut child = launcher.launch().unwrap();
        assert!(child.id() > 0);
        thread::sleep(Duration::from_millis(100));
        assert!(!child.exited());
        child.kill();
        assert!(child.exited());
    }
}
