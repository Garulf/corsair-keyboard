use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use engine::Spectrum;

use crate::{Pipeline, Samples, SpectrumCell};

pub const RETRY_AFTER: Duration = Duration::from_secs(3);
pub const DEVICE_CHECK_EVERY: Duration = Duration::from_secs(2);

const STOP_CHECK_EVERY: Duration = Duration::from_millis(50);

pub type SampleSink = Box<dyn FnMut(Samples<'_>) + Send>;

#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    Stopped,
    DeviceChanged,
}

pub trait Capture: Send + 'static {
    fn run(&mut self, stop: &AtomicBool, device: &mut dyn FnMut(String), sink: SampleSink) -> Result<Exit, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Starting,
    Capturing { device: String },
    Failed { reason: String },
}

pub struct AudioHandle {
    cell: Arc<SpectrumCell>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioHandle {
    pub fn spawn(capture: Box<dyn Capture>) -> AudioHandle {
        AudioHandle::spawn_with(capture, RETRY_AFTER)
    }

    pub(crate) fn spawn_with(capture: Box<dyn Capture>, retry_after: Duration) -> AudioHandle {
        let worker = Worker {
            capture,
            cell: Arc::new(SpectrumCell::new()),
            status: Arc::new(Mutex::new(Status::Starting)),
            stop: Arc::new(AtomicBool::new(false)),
            retry_after,
        };
        AudioHandle {
            cell: Arc::clone(&worker.cell),
            status: Arc::clone(&worker.status),
            stop: Arc::clone(&worker.stop),
            thread: Some(thread::spawn(move || worker.run())),
        }
    }

    pub fn spectrum(&self) -> Spectrum {
        self.cell.latest()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Worker {
    capture: Box<dyn Capture>,
    cell: Arc<SpectrumCell>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    retry_after: Duration,
}

impl Worker {
    fn run(mut self) {
        let mut announce_start = true;
        loop {
            if announce_start {
                set_status(&self.status, Status::Starting);
            }
            let mut pipeline = Pipeline::new(Arc::clone(&self.cell));
            let sink: SampleSink = Box::new(move |samples| pipeline.feed(samples));
            let status = Arc::clone(&self.status);
            let mut report_device = |device| set_status(&status, Status::Capturing { device });
            match self.capture.run(&self.stop, &mut report_device, sink) {
                Ok(Exit::Stopped) => return,
                Ok(Exit::DeviceChanged) => announce_start = true,
                Err(reason) => {
                    self.cell.publish(Spectrum::SILENT);
                    set_status(&self.status, Status::Failed { reason });
                    announce_start = false;
                    if !self.wait_to_retry() {
                        return;
                    }
                }
            }
        }
    }

    fn wait_to_retry(&self) -> bool {
        let retry_at = Instant::now() + self.retry_after;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            let remaining = retry_at.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return true;
            }
            thread::sleep(remaining.min(STOP_CHECK_EVERY));
        }
    }
}

fn set_status(status: &Mutex<Status>, next: Status) {
    *status.lock().unwrap() = next;
}

pub fn system_capture() -> Box<dyn Capture> {
    #[cfg(windows)]
    return Box::new(crate::wasapi::Wasapi);
    #[cfg(target_os = "linux")]
    return Box::new(crate::pulse::PulseMonitor);
    #[cfg(not(any(windows, target_os = "linux")))]
    return Box::new(Unsupported);
}

#[cfg(not(any(windows, target_os = "linux")))]
struct Unsupported;

#[cfg(not(any(windows, target_os = "linux")))]
impl Capture for Unsupported {
    fn run(&mut self, _: &AtomicBool, _: &mut dyn FnMut(String), _: SampleSink) -> Result<Exit, String> {
        Err("audio capture is not supported on this platform".to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::analyzer::{band_containing, sine};

    const RATE: u32 = 48_000;
    const QUICK_RETRY: Duration = Duration::from_millis(20);
    const SLOW_RETRY: Duration = Duration::from_millis(200);
    const NEVER_RETRY: Duration = Duration::from_secs(60);
    const DEADLINE: Duration = Duration::from_secs(1);
    const OPENING: Duration = Duration::from_millis(30);

    enum Step {
        Fail(&'static str),
        ChangeDevice,
    }

    struct Scripted {
        steps: VecDeque<Step>,
        feeds: bool,
        runs: Arc<AtomicUsize>,
    }

    impl Scripted {
        fn new(steps: impl IntoIterator<Item = Step>, feeds: bool) -> (Box<Scripted>, Arc<AtomicUsize>) {
            let runs = Arc::new(AtomicUsize::new(0));
            let scripted = Scripted { steps: steps.into_iter().collect(), feeds, runs: Arc::clone(&runs) };
            (Box::new(scripted), runs)
        }

        fn capture_briefly(&self, device: &mut dyn FnMut(String), sink: &mut SampleSink) {
            device("fake".to_string());
            self.feed(sink);
        }

        fn feed(&self, sink: &mut SampleSink) {
            if self.feeds {
                let data = sine(RATE, 1000.0, 1.0, 2, 8192);
                sink(Samples { data: &data, channels: 2, rate: RATE });
            }
        }
    }

    impl Capture for Scripted {
        fn run(
            &mut self,
            stop: &AtomicBool,
            device: &mut dyn FnMut(String),
            mut sink: SampleSink,
        ) -> Result<Exit, String> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            thread::sleep(OPENING);
            match self.steps.pop_front() {
                Some(Step::Fail(reason)) => {
                    self.feed(&mut sink);
                    Err(reason.to_string())
                }
                Some(Step::ChangeDevice) => {
                    self.capture_briefly(device, &mut sink);
                    thread::sleep(OPENING);
                    Ok(Exit::DeviceChanged)
                }
                None => {
                    self.capture_briefly(device, &mut sink);
                    while !stop.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(5));
                        self.feed(&mut sink);
                    }
                    Ok(Exit::Stopped)
                }
            }
        }
    }

    fn capturing_fake() -> Status {
        Status::Capturing { device: "fake".to_string() }
    }

    fn wait_until(handle: &AudioHandle, wanted: impl Fn(&Status, &Spectrum) -> bool) -> Vec<Status> {
        let mut seen: Vec<Status> = Vec::new();
        let deadline = Instant::now() + DEADLINE;
        loop {
            let status = handle.status();
            let done = wanted(&status, &handle.spectrum());
            if seen.last() != Some(&status) {
                seen.push(status);
            }
            if done {
                return seen;
            }
            assert!(Instant::now() < deadline, "timed out; statuses seen: {seen:?}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_running_capture_reports_its_device_and_publishes_bands() {
        let (capture, _) = Scripted::new([], true);
        let handle = AudioHandle::spawn_with(capture, QUICK_RETRY);
        let tone_band = band_containing(1000.0);

        wait_until(&handle, |status, spectrum| *status == capturing_fake() && spectrum.bands[tone_band] >= 0.9);
    }

    #[test]
    fn a_failure_is_reported_then_retried_until_capture_recovers() {
        let (capture, runs) = Scripted::new([Step::Fail("boom")], false);
        let handle = AudioHandle::spawn_with(capture, SLOW_RETRY);

        let seen = wait_until(&handle, |status, _| *status == capturing_fake() && runs.load(Ordering::SeqCst) == 2);

        assert!(seen.contains(&Status::Failed { reason: "boom".to_string() }), "{seen:?}");
    }

    #[test]
    fn the_first_attempt_reports_starting() {
        let (capture, _) = Scripted::new([Step::Fail("boom")], false);
        let handle = AudioHandle::spawn_with(capture, NEVER_RETRY);

        let seen = wait_until(&handle, |status, _| matches!(status, Status::Failed { .. }));

        assert_eq!(seen, [Status::Starting, Status::Failed { reason: "boom".to_string() }]);
    }

    #[test]
    fn a_persistent_failure_stays_failed_between_retries() {
        let (capture, runs) = Scripted::new([Step::Fail("first"), Step::Fail("second")], false);
        let handle = AudioHandle::spawn_with(capture, QUICK_RETRY);

        let seen = wait_until(&handle, |status, _| *status == capturing_fake() && runs.load(Ordering::SeqCst) == 3);

        assert_eq!(
            seen,
            [
                Status::Starting,
                Status::Failed { reason: "first".to_string() },
                Status::Failed { reason: "second".to_string() },
                capturing_fake(),
            ]
        );
    }

    #[test]
    fn a_device_change_reports_starting_again() {
        let (capture, runs) = Scripted::new([Step::ChangeDevice], false);
        let handle = AudioHandle::spawn_with(capture, QUICK_RETRY);

        let seen = wait_until(&handle, |status, _| *status == capturing_fake() && runs.load(Ordering::SeqCst) == 2);

        assert_eq!(seen, [Status::Starting, capturing_fake(), Status::Starting, capturing_fake()]);
    }

    #[test]
    fn a_device_change_restarts_without_reporting_a_failure() {
        let (capture, runs) = Scripted::new([Step::ChangeDevice], false);
        let handle = AudioHandle::spawn_with(capture, QUICK_RETRY);

        let seen = wait_until(&handle, |status, _| *status == capturing_fake() && runs.load(Ordering::SeqCst) == 2);

        assert!(!seen.iter().any(|status| matches!(status, Status::Failed { .. })), "{seen:?}");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn dropping_the_handle_stops_and_joins_the_thread() {
        let (capture, _) = Scripted::new([], false);
        let handle = AudioHandle::spawn_with(capture, QUICK_RETRY);
        wait_until(&handle, |status, _| *status == capturing_fake());

        let dropping = Instant::now();
        drop(handle);

        assert!(dropping.elapsed() < DEADLINE, "drop took {:?}", dropping.elapsed());
    }

    #[test]
    fn dropping_the_handle_while_waiting_to_retry_returns_promptly() {
        let (capture, _) = Scripted::new([Step::Fail("boom")], false);
        let handle = AudioHandle::spawn_with(capture, NEVER_RETRY);
        wait_until(&handle, |status, _| matches!(status, Status::Failed { .. }));

        let dropping = Instant::now();
        drop(handle);

        assert!(dropping.elapsed() < DEADLINE, "drop took {:?}", dropping.elapsed());
    }

    #[test]
    fn a_failure_clears_the_published_spectrum() {
        let (capture, _) = Scripted::new([Step::Fail("boom")], true);
        let handle = AudioHandle::spawn_with(capture, NEVER_RETRY);

        wait_until(&handle, |status, _| matches!(status, Status::Failed { .. }));

        assert_eq!(handle.spectrum(), Spectrum::SILENT);
    }
}
