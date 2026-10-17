use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread;

use interprocess::local_socket::{Listener, ListenerOptions, Name, Stream, prelude::*};
use ipc::{InstanceLock, MAX_LINE, Reply, Request, write_message};

use crate::runtime::{Command, SUBSCRIBER_QUEUE};

pub fn bind(lock: &InstanceLock) -> io::Result<Listener> {
    let listener = bind_name(ipc::socket_name()?, ipc::socket_path().as_deref(), lock)?;
    restrict_socket_permissions()?;
    Ok(listener)
}

pub fn bind_name(name: Name<'static>, stale_socket: Option<&Path>, _lock: &InstanceLock) -> io::Result<Listener> {
    if Stream::connect(name.borrow()).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "k100d is already running"));
    }
    if name.is_path()
        && let Some(path) = stale_socket
    {
        let _ = std::fs::remove_file(path);
    }
    ListenerOptions::new().name(name).create_sync()
}

#[cfg(unix)]
fn restrict_socket_permissions() -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match ipc::socket_path() {
        Some(path) => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn restrict_socket_permissions() -> io::Result<()> {
    Ok(())
}

pub fn serve(listener: Listener, commands: Sender<Command>) {
    for stream in listener.incoming().filter_map(Result::ok) {
        let commands = commands.clone();
        thread::spawn(move || {
            let _ = handle_client(stream, &commands);
        });
    }
}

fn handle_client(stream: Stream, commands: &Sender<Command>) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        let read = reader.by_ref().take(MAX_LINE as u64 + 1).read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        if read > MAX_LINE {
            write_message(reader.get_mut(), &Reply::Error { message: "message too long".into() })?;
            return Ok(());
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                write_message(reader.get_mut(), &Reply::Error { message: format!("bad request: {error}") })?;
                continue;
            }
        };
        let subscription = match request {
            Request::Subscribe => Some(false),
            Request::SubscribeFrames => Some(true),
            _ => None,
        };
        if let Some(frames) = subscription {
            let (events_tx, events_rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
            if commands.send(Command::Subscribe { events: events_tx, frames }).is_err() {
                return Ok(());
            }
            write_message(reader.get_mut(), &Reply::Done)?;
            for event in events_rx {
                write_message(reader.get_mut(), &event)?;
            }
            return Ok(());
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if commands.send(Command::Request(request, reply_tx)).is_err() {
            return Ok(());
        }
        let reply = reply_rx.recv().unwrap_or(Reply::Error { message: "daemon is shutting down".into() });
        write_message(reader.get_mut(), &reply)?;
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;
    use std::time::Duration;

    use interprocess::local_socket::{GenericNamespaced, Stream};
    use ipc::{Client, PROTOCOL_VERSION, Reply, Request};

    use super::*;

    struct Instance {
        _folder: tempfile::TempDir,
        _lock: InstanceLock,
    }

    fn start(name: &'static str) -> (mpsc::Receiver<Command>, Instance) {
        let folder = tempfile::tempdir().unwrap();
        let lock = InstanceLock::acquire(&folder.path().join("k100d.lock")).unwrap();
        let listener = bind_name(name.to_ns_name::<GenericNamespaced>().unwrap(), None, &lock).unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || serve(listener, sender));
        (receiver, Instance { _folder: folder, _lock: lock })
    }

    fn answer(receiver: &mpsc::Receiver<Command>, reply: Reply) {
        match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
            Command::Request(_, tx) => tx.send(reply).unwrap(),
            _ => panic!("expected a request"),
        }
    }

    #[test]
    fn forwards_requests_and_replies() {
        let (receiver, _instance) = start("k100d-server-test-forward");
        let client = std::thread::spawn(|| {
            let name = "k100d-server-test-forward".to_ns_name::<GenericNamespaced>().unwrap();
            let mut client = Client::connect_to(name).unwrap();
            client.request(&Request::ListProfiles).unwrap()
        });
        answer(&receiver, Reply::Hello { version: PROTOCOL_VERSION });
        answer(&receiver, Reply::Profiles { names: vec!["default".into()] });
        assert_eq!(client.join().unwrap(), Reply::Profiles { names: vec!["default".into()] });
    }

    #[test]
    fn garbage_gets_an_error_and_the_connection_survives() {
        let (receiver, _instance) = start("k100d-server-test-garbage");
        let stream = Stream::connect("k100d-server-test-garbage".to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
        let mut reader = BufReader::new(stream);
        reader.get_mut().write_all(b"{nonsense}\n").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("\"error\""), "{line}");
        reader.get_mut().write_all(b"{\"type\":\"get_state\"}\n").unwrap();
        let next = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(next, Command::Request(Request::GetState, _)));
    }

    #[test]
    fn subscribers_receive_events() {
        let (receiver, _instance) = start("k100d-server-test-events");
        let stream = Stream::connect("k100d-server-test-events".to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
        let mut reader = BufReader::new(stream);
        reader.get_mut().write_all(b"{\"type\":\"subscribe\"}\n").unwrap();
        let Command::Subscribe { events, frames } = receiver.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!("expected subscribe");
        };
        assert!(!frames);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("\"done\""));
        events.send(Reply::Event { event: ipc::Event::ProfileChanged { name: "games".into() } }).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("profile_changed"), "{line}");
    }

    #[test]
    fn frame_subscribers_are_marked() {
        let (receiver, _instance) = start("k100d-server-test-frames");
        let stream = Stream::connect("k100d-server-test-frames".to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
        let mut reader = BufReader::new(stream);
        reader.get_mut().write_all(b"{\"type\":\"subscribe_frames\"}\n").unwrap();
        let Command::Subscribe { frames, .. } = receiver.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!("expected subscribe");
        };
        assert!(frames);
    }

    #[test]
    fn a_second_daemon_cannot_bind() {
        let (_receiver, instance) = start("k100d-server-test-twice");
        let name = "k100d-server-test-twice".to_ns_name::<GenericNamespaced>().unwrap();
        let error = bind_name(name, None, &instance._lock).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_second_daemon_leaves_the_first_socket_alone() {
        use interprocess::local_socket::GenericFilePath;

        let folder = tempfile::tempdir().unwrap();
        let socket = folder.path().join("k100d.sock");
        let lock_file = folder.path().join("k100d.lock");
        let name = || socket.clone().to_fs_name::<GenericFilePath>().unwrap();

        let first = InstanceLock::acquire(&lock_file).unwrap();
        let _listener = bind_name(name(), Some(&socket), &first).unwrap();

        let refused = InstanceLock::acquire(&lock_file).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(refused.to_string().contains("already running"), "{refused}");
        assert!(socket.exists());
        assert!(Stream::connect(name()).is_ok());
    }
}
