use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::player::Player;
use crate::program::Program;
use crate::sink::{InputSink, SinkEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputCommand {
    Press { source: u8, program: Program },
    Release { source: u8 },
    Stop { source: u8 },
    Emit(Vec<SinkEvent>),
    CancelAll,
    Launch { program: String, args: Vec<String> },
    OpenUrl { url: String },
}

#[derive(Clone)]
pub struct OutputHandle {
    sender: Sender<OutputCommand>,
    busy: Arc<AtomicBool>,
}

impl OutputHandle {
    pub fn send(&self, command: OutputCommand) {
        let _ = self.sender.send(command);
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
}

pub fn spawn(sink: Box<dyn InputSink>) -> (OutputHandle, JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel();
    let busy = Arc::new(AtomicBool::new(false));
    let worker_busy = busy.clone();
    let thread = thread::spawn(move || run(sink, receiver, &worker_busy));
    (OutputHandle { sender, busy }, thread)
}

fn run(mut sink: Box<dyn InputSink>, commands: Receiver<OutputCommand>, busy: &AtomicBool) {
    let started = Instant::now();
    let mut player = Player::new(sink.needs_typematic());
    loop {
        let received = match player.next_deadline() {
            Some(deadline) => commands.recv_timeout(deadline.saturating_sub(started.elapsed())),
            None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let now = started.elapsed();
        let mut events = match received {
            Ok(command) => apply(&mut player, command, now),
            Err(RecvTimeoutError::Timeout) => Vec::new(),
            Err(RecvTimeoutError::Disconnected) => {
                deliver(sink.as_mut(), &player.cancel_all());
                busy.store(false, Ordering::Release);
                return;
            }
        };
        events.extend(player.poll(now));
        deliver(sink.as_mut(), &events);
        busy.store(!player.is_idle(), Ordering::Release);
    }
}

fn apply(player: &mut Player, command: OutputCommand, now: Duration) -> Vec<SinkEvent> {
    match command {
        OutputCommand::Press { source, program } => player.press(source, program, now),
        OutputCommand::Release { source } => player.release(source, now),
        OutputCommand::Stop { source } => player.stop_source(source),
        OutputCommand::Emit(events) => events,
        OutputCommand::CancelAll => player.cancel_all(),
        OutputCommand::Launch { program, args } => {
            launch(&program, &args);
            Vec::new()
        }
        OutputCommand::OpenUrl { url } => {
            open_url(&url);
            Vec::new()
        }
    }
}

fn deliver(sink: &mut dyn InputSink, events: &[SinkEvent]) {
    for event in events {
        if let Err(error) = sink.send(event) {
            eprintln!("k100d: sending input failed: {error}");
        }
    }
}

fn launch(program: &str, args: &[String]) {
    match std::process::Command::new(program).args(args).spawn() {
        Ok(mut child) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => eprintln!("k100d: cannot launch {program}: {error}"),
    }
}

#[cfg(windows)]
fn open_url(url: &str) {
    launch("rundll32.exe", &["url.dll,FileProtocolHandler".to_owned(), url.to_owned()]);
}

#[cfg(not(windows))]
fn open_url(url: &str) {
    launch("xdg-open", &[url.to_owned()]);
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::config::{MacroStep, RepeatMode};
    use crate::keys::Key;
    use crate::sink::RecordingSink;

    fn key(name: &str) -> Key {
        Key::named(name).unwrap()
    }

    fn down(name: &str) -> SinkEvent {
        SinkEvent::key(key(name), true)
    }

    fn up(name: &str) -> SinkEvent {
        SinkEvent::key(key(name), false)
    }

    #[test]
    fn plays_commands_in_order_and_releases_on_shutdown() {
        let sink = RecordingSink::default();
        let (handle, thread) = spawn(Box::new(sink.clone()));
        handle.send(OutputCommand::Press { source: 4, program: Program::remap(key("b"), &[]) });
        handle.send(OutputCommand::Release { source: 4 });
        handle.send(OutputCommand::Emit(vec![SinkEvent::Text("hi".into())]));
        let held = Program::macro_steps(&[MacroStep::Down(key("c")), MacroStep::Delay(10_000)], RepeatMode::Once);
        handle.send(OutputCommand::Press { source: 5, program: held });
        drop(handle);
        thread.join().unwrap();
        assert_eq!(sink.take(), [down("b"), up("b"), SinkEvent::Text("hi".into()), down("c"), up("c")]);
    }

    #[test]
    fn stop_ends_what_a_source_plays() {
        let sink = RecordingSink::default();
        let (handle, thread) = spawn(Box::new(sink.clone()));
        let toggle = Program::macro_steps(&[MacroStep::Down(key("c")), MacroStep::Delay(10_000)], RepeatMode::Toggle);
        handle.send(OutputCommand::Press { source: 5, program: toggle });
        handle.send(OutputCommand::Stop { source: 5 });
        handle.send(OutputCommand::Emit(vec![SinkEvent::Text("after".into())]));
        assert_eq!(collect(&sink, 3, Duration::from_secs(5)), [down("c"), up("c"), SinkEvent::Text("after".into())]);
        drop(handle);
        thread.join().unwrap();
        assert_eq!(sink.take(), []);
    }

    fn collect(sink: &RecordingSink, count: usize, within: Duration) -> Vec<SinkEvent> {
        let deadline = Instant::now() + within;
        let mut events = Vec::new();
        while events.len() < count && Instant::now() < deadline {
            events.extend(sink.take());
            std::thread::sleep(Duration::from_millis(5));
        }
        events
    }

    fn wait_until(within: Duration, condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        condition()
    }

    #[test]
    fn reports_busy_while_a_macro_plays() {
        let sink = RecordingSink::default();
        let (handle, thread) = spawn(Box::new(sink.clone()));
        assert!(!handle.is_busy());
        let steps = [MacroStep::Tap(key("a")), MacroStep::Delay(300), MacroStep::Tap(key("b"))];
        handle.send(OutputCommand::Press { source: 1, program: Program::macro_steps(&steps, RepeatMode::Once) });
        assert!(wait_until(Duration::from_secs(5), || handle.is_busy()), "busy while the delay runs");
        assert!(wait_until(Duration::from_secs(5), || !handle.is_busy()), "idle once the macro ends");
        assert_eq!(sink.take(), [down("a"), up("a"), down("b"), up("b")]);
        drop(handle);
        thread.join().unwrap();
    }

    #[test]
    fn waits_out_macro_delays() {
        let delay = Duration::from_millis(400);
        let sink = RecordingSink::default();
        let (handle, thread) = spawn(Box::new(sink.clone()));
        let steps = [MacroStep::Tap(key("a")), MacroStep::Delay(400), MacroStep::Tap(key("b"))];
        let pressed = Instant::now();
        handle.send(OutputCommand::Press { source: 1, program: Program::macro_steps(&steps, RepeatMode::Once) });
        assert_eq!(collect(&sink, 2, Duration::from_secs(5)), [down("a"), up("a")]);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(sink.take(), []);
        assert_eq!(collect(&sink, 2, Duration::from_secs(5)), [down("b"), up("b")]);
        assert!(pressed.elapsed() >= delay);
        drop(handle);
        thread.join().unwrap();
    }
}
