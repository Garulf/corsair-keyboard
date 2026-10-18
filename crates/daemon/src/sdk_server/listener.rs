use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use ipc::SdkStatus;
use sdk::ClientId;

use crate::runtime::Command;
use crate::sdk_server::SdkCommand;
use crate::sdk_server::client::{AcceptedClient, serve_client};

pub const IN_USE_RETRY: Duration = Duration::from_secs(10);
const CLIENT_POLL: Duration = Duration::from_millis(500);
const STEP: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    InUse,
    Failed(String),
}

pub trait PipeInstance: Send {
    fn wait_for_client(&mut self, timeout: Duration) -> io::Result<bool>;
    fn accept(self) -> io::Result<AcceptedClient>;
}

pub trait PipeFactory: Send {
    type Instance: PipeInstance;
    fn create(&mut self, first: bool) -> Result<Self::Instance, CreateError>;
}

pub fn listen<F: PipeFactory>(factory: F, commands: Sender<Command>, enabled: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    listen_with(factory, commands, enabled, stop, IN_USE_RETRY);
}

pub fn listen_with<F: PipeFactory>(
    mut factory: F,
    commands: Sender<Command>,
    enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    retry: Duration,
) {
    let mut listener = Listener { commands, enabled, stop, retry, next_client: 1, off_reported: false };
    let mut pending: Option<F::Instance> = None;
    while !listener.stopped() {
        if !listener.enabled() {
            pending = None;
            listener.report_off();
            thread::sleep(STEP);
            continue;
        }
        listener.off_reported = false;
        let mut instance = match pending.take() {
            Some(instance) => instance,
            None => match factory.create(true) {
                Ok(instance) => {
                    listener.report(SdkStatus::Listening);
                    instance
                }
                Err(error) => {
                    listener.report(status_of(error));
                    listener.pause();
                    continue;
                }
            },
        };
        match instance.wait_for_client(CLIENT_POLL) {
            Ok(true) => {}
            Ok(false) => {
                pending = Some(instance);
                continue;
            }
            Err(error) => {
                listener.report(SdkStatus::Failed { reason: format!("waiting for an SDK client: {error}") });
                listener.pause();
                continue;
            }
        }
        pending = factory.create(false).map_err(|error| listener.report(status_of(error))).ok();
        listener.serve(instance);
        if pending.is_none() {
            listener.pause();
        }
    }
}

fn status_of(error: CreateError) -> SdkStatus {
    match error {
        CreateError::InUse => SdkStatus::InUse,
        CreateError::Failed(reason) => SdkStatus::Failed { reason },
    }
}

struct Listener {
    commands: Sender<Command>,
    enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    retry: Duration,
    next_client: ClientId,
    off_reported: bool,
}

impl Listener {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    fn report(&self, status: SdkStatus) {
        let _ = self.commands.send(Command::Sdk(SdkCommand::Status(status)));
    }

    fn report_off(&mut self) {
        if !self.off_reported {
            self.off_reported = true;
            self.report(SdkStatus::Off);
        }
    }

    fn pause(&self) {
        let until = Instant::now() + self.retry;
        while !self.stopped() && self.enabled() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            thread::sleep(left.min(STEP));
        }
    }

    fn serve(&mut self, instance: impl PipeInstance) {
        match instance.accept() {
            Ok(accepted) => {
                let client = self.next_client;
                self.next_client += 1;
                serve_client(client, accepted, self.commands.clone());
            }
            Err(error) => eprintln!("k100d: could not accept an iCUE SDK client: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use ipc::SdkStatus;

    use super::*;
    use crate::runtime::Command;
    use crate::sdk_server::SdkCommand;
    use crate::sdk_server::client::AcceptedClient;

    #[derive(Clone, Default)]
    struct Script {
        outcomes: Arc<Mutex<VecDeque<Result<(), CreateError>>>>,
        clients: Arc<Mutex<VecDeque<AcceptedClient>>>,
        creates: Arc<Mutex<Vec<bool>>>,
        dropped: Arc<AtomicUsize>,
    }

    impl Script {
        fn failing(outcomes: Vec<CreateError>) -> Self {
            let script = Self::default();
            script.outcomes.lock().unwrap().extend(outcomes.into_iter().map(Err));
            script
        }

        fn creates(&self) -> Vec<bool> {
            self.creates.lock().unwrap().clone()
        }
    }

    struct FakeFactory(Script);

    struct FakeInstance {
        script: Script,
        client: Option<AcceptedClient>,
    }

    impl Drop for FakeInstance {
        fn drop(&mut self) {
            self.script.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl PipeFactory for FakeFactory {
        type Instance = FakeInstance;

        fn create(&mut self, first: bool) -> Result<FakeInstance, CreateError> {
            self.0.creates.lock().unwrap().push(first);
            let outcome = self.0.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()));
            outcome.map(|()| FakeInstance { script: self.0.clone(), client: None })
        }
    }

    impl PipeInstance for FakeInstance {
        fn wait_for_client(&mut self, timeout: Duration) -> io::Result<bool> {
            self.client = self.script.clients.lock().unwrap().pop_front();
            if self.client.is_none() {
                thread::sleep(timeout.min(Duration::from_millis(5)));
            }
            Ok(self.client.is_some())
        }

        fn accept(mut self) -> io::Result<AcceptedClient> {
            self.client.take().ok_or_else(|| io::Error::other("no client"))
        }
    }

    fn client(exe: &str) -> AcceptedClient {
        AcceptedClient {
            exe: exe.into(),
            reader: Box::new(io::empty()),
            writer: Box::new(io::sink()),
            hang_up: Box::new(|| {}),
        }
    }

    struct Running {
        commands: Receiver<Command>,
        enabled: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        thread: JoinHandle<()>,
    }

    impl Running {
        fn next_status(&self) -> SdkStatus {
            loop {
                if let Command::Sdk(SdkCommand::Status(status)) =
                    self.commands.recv_timeout(Duration::from_secs(5)).unwrap()
                {
                    return status;
                }
            }
        }

        fn next_connected(&self) -> (ClientId, String) {
            loop {
                if let Command::Sdk(SdkCommand::Connected { client, exe, .. }) =
                    self.commands.recv_timeout(Duration::from_secs(5)).unwrap()
                {
                    return (client, exe);
                }
            }
        }

        fn finish(self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.join().unwrap();
        }
    }

    fn start(script: &Script, enabled: bool) -> Running {
        let (sender, commands) = mpsc::channel();
        let enabled = Arc::new(AtomicBool::new(enabled));
        let stop = Arc::new(AtomicBool::new(false));
        let factory = FakeFactory(script.clone());
        let (gate, flag) = (enabled.clone(), stop.clone());
        let thread = thread::spawn(move || listen_with(factory, sender, gate, flag, Duration::from_millis(10)));
        Running { commands, enabled, stop, thread }
    }

    #[test]
    fn a_taken_name_reports_in_use_and_retries_until_free() {
        let script = Script::failing(vec![CreateError::InUse, CreateError::InUse]);
        let running = start(&script, true);
        assert_eq!(running.next_status(), SdkStatus::InUse);
        assert_eq!(running.next_status(), SdkStatus::InUse);
        assert_eq!(running.next_status(), SdkStatus::Listening);
        assert_eq!(script.creates(), [true, true, true]);
        running.finish();
    }

    #[test]
    fn a_failed_create_reports_the_reason_and_retries() {
        let script = Script::failing(vec![CreateError::Failed("denied".into())]);
        let running = start(&script, true);
        assert_eq!(running.next_status(), SdkStatus::Failed { reason: "denied".into() });
        assert_eq!(running.next_status(), SdkStatus::Listening);
        running.finish();
    }

    #[test]
    fn each_client_gets_its_own_instance_and_id() {
        let script = Script::default();
        script.clients.lock().unwrap().extend([client("a.exe"), client("b.exe")]);
        let running = start(&script, true);
        assert_eq!(running.next_connected(), (1, "a.exe".into()));
        assert_eq!(running.next_connected(), (2, "b.exe".into()));
        assert_eq!(script.creates(), [true, false, false]);
        running.finish();
    }

    #[test]
    fn stopping_ends_the_loop() {
        let script = Script::default();
        let running = start(&script, true);
        assert_eq!(running.next_status(), SdkStatus::Listening);
        let started = Instant::now();
        running.finish();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_disabled_sdk_owns_no_pipe_until_enabled() {
        let script = Script::default();
        let running = start(&script, false);
        assert_eq!(running.next_status(), SdkStatus::Off);
        thread::sleep(Duration::from_millis(150));
        assert!(script.creates().is_empty());
        running.enabled.store(true, Ordering::SeqCst);
        assert_eq!(running.next_status(), SdkStatus::Listening);
        assert_eq!(script.creates(), [true]);
        running.enabled.store(false, Ordering::SeqCst);
        assert_eq!(running.next_status(), SdkStatus::Off);
        assert_eq!(script.dropped.load(Ordering::SeqCst), 1, "the pending instance is let go");
        running.finish();
    }
}
