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
    Emit(Vec<SinkEvent>),
    CancelAll,
    Launch { program: String, args: Vec<String> },
    OpenUrl { url: String },
}

#[derive(Clone)]
pub struct OutputHandle {
    sender: Sender<OutputCommand>,
}

impl OutputHandle {
    pub fn send(&self, command: OutputCommand) {
        let _ = self.sender.send(command);
    }
}

pub fn spawn(sink: Box<dyn InputSink>) -> (OutputHandle, JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel();
    let thread = thread::spawn(move || run(sink, receiver));
    (OutputHandle { sender }, thread)
}

fn run(mut sink: Box<dyn InputSink>, commands: Receiver<OutputCommand>) {
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
                return;
            }
        };
        events.extend(player.poll(now));
        deliver(sink.as_mut(), &events);
    }
}

fn apply(player: &mut Player, command: OutputCommand, now: Duration) -> Vec<SinkEvent> {
    match command {
        OutputCommand::Press { source, program } => player.press(source, program, now),
        OutputCommand::Release { source } => player.release(source, now),
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
    use std::time::Duration;

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
        std::thread::sleep(Duration::from_millis(100));
        drop(handle);
        thread.join().unwrap();
        assert_eq!(sink.take(), [down("b"), up("b"), SinkEvent::Text("hi".into()), down("c"), up("c")]);
    }

    #[test]
    fn waits_out_macro_delays() {
        let sink = RecordingSink::default();
        let (handle, thread) = spawn(Box::new(sink.clone()));
        let steps = [MacroStep::Tap(key("a")), MacroStep::Delay(60), MacroStep::Tap(key("b"))];
        handle.send(OutputCommand::Press { source: 1, program: Program::macro_steps(&steps, RepeatMode::Once) });
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(sink.take(), [down("a"), up("a")]);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(sink.take(), [down("b"), up("b")]);
        drop(handle);
        thread.join().unwrap();
    }
}
