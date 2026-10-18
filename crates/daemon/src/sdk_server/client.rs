use std::io::{ErrorKind, Read, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, sync_channel};
use std::thread;

use sdk::{ClientId, FrameDecoder, encode_frame};

use crate::runtime::Command;
use crate::sdk_server::{HangUp, OUTBOX_QUEUE, SdkCommand};

const READ_CHUNK: usize = 64 * 1024;

pub struct AcceptedClient {
    pub exe: String,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub hang_up: HangUp,
}

pub fn serve_client<Live: Send + Sync + 'static>(
    client: ClientId,
    accepted: AcceptedClient,
    commands: Sender<Command>,
    live: Live,
) {
    let AcceptedClient { exe, reader, writer, hang_up } = accepted;
    let (outbox, queue) = sync_channel(OUTBOX_QUEUE);
    let connected = SdkCommand::Connected { client, exe, outbox, hang_up: hang_up.clone() };
    if commands.send(Command::Sdk(connected)).is_err() {
        hang_up();
        return;
    }
    let live = Arc::new(live);
    let reader_live = live.clone();
    thread::spawn(move || {
        read_frames(client, reader, &commands);
        let _ = commands.send(Command::Sdk(SdkCommand::Disconnected { client }));
        drop(reader_live);
    });
    thread::spawn(move || {
        write_frames(writer, queue);
        hang_up();
        drop(live);
    });
}

fn read_frames(client: ClientId, mut reader: Box<dyn Read + Send>, commands: &Sender<Command>) {
    let mut decoder = FrameDecoder::default();
    let mut buffer = vec![0; READ_CHUNK];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        decoder.push(&buffer[..read]);
        if !forward_frames(client, &mut decoder, commands) {
            return;
        }
    }
}

fn forward_frames(client: ClientId, decoder: &mut FrameDecoder, commands: &Sender<Command>) -> bool {
    loop {
        match decoder.next_frame() {
            Ok(Some(payload)) => {
                if commands.send(Command::Sdk(SdkCommand::Message { client, payload })).is_err() {
                    return false;
                }
            }
            Ok(None) => return true,
            Err(_) => return false,
        }
    }
}

fn write_frames(mut writer: Box<dyn Write + Send>, queue: Receiver<Vec<u8>>) {
    for payload in queue {
        if writer.write_all(&encode_frame(&payload)).and_then(|()| writer.flush()).is_err() {
            return;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc::{self, Receiver, SyncSender};
    use std::time::Duration;

    use super::*;
    use crate::runtime::Command;
    use crate::sdk_server::SdkCommand;

    fn accepted(stream: &UnixStream) -> AcceptedClient {
        let closer = stream.try_clone().unwrap();
        AcceptedClient {
            exe: "game.exe".into(),
            reader: Box::new(stream.try_clone().unwrap()),
            writer: Box::new(stream.try_clone().unwrap()),
            hang_up: Arc::new(move || {
                let _ = closer.shutdown(Shutdown::Both);
            }),
        }
    }

    fn next_sdk(receiver: &Receiver<Command>) -> SdkCommand {
        match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
            Command::Sdk(command) => command,
            _ => panic!("expected an SDK command"),
        }
    }

    fn connect(
        client: ClientId,
        commands: &mpsc::Sender<Command>,
        receiver: &Receiver<Command>,
    ) -> (UnixStream, SyncSender<Vec<u8>>) {
        let (peer, outbox, _) = connect_with_hang_up(client, commands, receiver, ());
        (peer, outbox)
    }

    fn connect_with_hang_up<Live: Send + Sync + 'static>(
        client: ClientId,
        commands: &mpsc::Sender<Command>,
        receiver: &Receiver<Command>,
        live: Live,
    ) -> (UnixStream, SyncSender<Vec<u8>>, HangUp) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        serve_client(client, accepted(&ours), commands.clone(), live);
        let SdkCommand::Connected { client: connected, exe, outbox, hang_up } = next_sdk(receiver) else {
            panic!("expected a connection")
        };
        assert_eq!((connected, exe.as_str()), (client, "game.exe"));
        (theirs, outbox, hang_up)
    }

    struct Live(mpsc::Sender<()>);

    impl Drop for Live {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn the_hang_up_handle_frees_a_writer_stuck_on_a_client_that_never_reads() {
        let (commands, receiver) = mpsc::channel();
        let (ended, end) = mpsc::channel();
        let (_peer, outbox, hang_up) = connect_with_hang_up(1, &commands, &receiver, Live(ended));
        for _ in 0..16 {
            outbox.send(vec![b'x'; 1024 * 1024]).unwrap();
        }
        std::thread::sleep(Duration::from_millis(200));
        drop(outbox);
        assert!(end.recv_timeout(Duration::from_millis(300)).is_err(), "the writer is stuck in write");
        hang_up();
        assert!(matches!(next_sdk(&receiver), SdkCommand::Disconnected { client: 1 }));
        end.recv_timeout(Duration::from_secs(5)).expect("both client threads end");
    }

    #[test]
    fn frames_from_a_client_reach_the_runtime_in_order() {
        let (commands, receiver) = mpsc::channel();
        let (mut peer, _outbox) = connect(1, &commands, &receiver);
        peer.write_all(&[sdk::encode_frame(b"first"), sdk::encode_frame(b"second")].concat()).unwrap();
        for expected in [&b"first"[..], b"second"] {
            let SdkCommand::Message { client, payload } = next_sdk(&receiver) else { panic!("expected a message") };
            assert_eq!((client, payload.as_slice()), (1, expected));
        }
        drop(peer);
        assert!(matches!(next_sdk(&receiver), SdkCommand::Disconnected { client: 1 }));
    }

    #[test]
    fn replies_are_framed_back_to_the_client() {
        let (commands, receiver) = mpsc::channel();
        let (mut peer, outbox) = connect(1, &commands, &receiver);
        outbox.send(b"{}".to_vec()).unwrap();
        let mut reply = [0u8; 6];
        peer.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0, 0, 0, 2, b'{', b'}']);
    }

    #[test]
    fn an_oversized_length_disconnects_only_that_client() {
        let (commands, receiver) = mpsc::channel();
        let (mut bad, _bad_outbox) = connect(1, &commands, &receiver);
        let (mut good, _good_outbox) = connect(2, &commands, &receiver);
        bad.write_all(&[0xFF; 4]).unwrap();
        assert!(matches!(next_sdk(&receiver), SdkCommand::Disconnected { client: 1 }));
        good.write_all(&sdk::encode_frame(b"still here")).unwrap();
        let SdkCommand::Message { client, payload } = next_sdk(&receiver) else { panic!("expected a message") };
        assert_eq!((client, payload.as_slice()), (2, &b"still here"[..]));
    }

    #[test]
    fn closing_the_outbox_hangs_up_the_client() {
        let (commands, receiver) = mpsc::channel();
        let (mut peer, outbox) = connect(1, &commands, &receiver);
        outbox.send(b"bye".to_vec()).unwrap();
        drop(outbox);
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut received = Vec::new();
        peer.read_to_end(&mut received).unwrap();
        assert_eq!(received, sdk::encode_frame(b"bye"));
        assert!(matches!(next_sdk(&receiver), SdkCommand::Disconnected { client: 1 }));
    }
}
