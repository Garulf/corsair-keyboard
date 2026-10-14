use std::io::{self, BufReader};
use std::path::PathBuf;

use interprocess::local_socket::{Name, Stream, prelude::*};

use crate::framing::{read_message, write_message};
use crate::{Event, PROTOCOL_VERSION, Reply, Request};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("cannot reach k100d: {0}")]
    Io(#[from] io::Error),
    #[error("k100d closed the connection")]
    Closed,
    #[error("k100d speaks protocol {daemon}, this client speaks {PROTOCOL_VERSION}")]
    VersionMismatch { daemon: u32 },
    #[error("unexpected reply: {0}")]
    Unexpected(String),
}

#[cfg(windows)]
pub fn socket_name() -> io::Result<Name<'static>> {
    use interprocess::local_socket::GenericNamespaced;
    "k100d".to_ns_name::<GenericNamespaced>()
}

#[cfg(not(windows))]
pub fn socket_name() -> io::Result<Name<'static>> {
    use interprocess::local_socket::GenericFilePath;
    let path = socket_path().ok_or_else(|| io::Error::other("no socket path"))?;
    path.to_fs_name::<GenericFilePath>()
}

pub fn socket_path() -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    Some(dir.join("k100d.sock"))
}

#[derive(Debug)]
pub struct Client {
    reader: BufReader<Stream>,
}

impl Client {
    pub fn connect() -> Result<Self, ClientError> {
        Self::connect_to(socket_name()?)
    }

    pub fn connect_to(name: Name<'_>) -> Result<Self, ClientError> {
        let mut client = Self { reader: BufReader::new(Stream::connect(name)?) };
        match client.request(&Request::Hello { version: PROTOCOL_VERSION })? {
            Reply::Hello { version } if version == PROTOCOL_VERSION => Ok(client),
            Reply::Hello { version } => Err(ClientError::VersionMismatch { daemon: version }),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    pub fn request(&mut self, request: &Request) -> Result<Reply, ClientError> {
        write_message(self.reader.get_mut(), request)?;
        read_message(&mut self.reader)?.ok_or(ClientError::Closed)
    }

    pub fn next_event(&mut self) -> Result<Option<Event>, ClientError> {
        match read_message(&mut self.reader)? {
            Some(Reply::Event { event }) => Ok(Some(event)),
            Some(other) => Err(ClientError::Unexpected(format!("{other:?}"))),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Write};

    use interprocess::local_socket::{GenericNamespaced, ListenerOptions};

    use super::*;
    use crate::framing::{read_message, write_message};

    fn fake_daemon(name: &'static str, version: u32) -> std::thread::JoinHandle<()> {
        let listener =
            ListenerOptions::new().name(name.to_ns_name::<GenericNamespaced>().unwrap()).create_sync().unwrap();
        std::thread::spawn(move || {
            let stream = listener.incoming().next().unwrap().unwrap();
            let mut reader = BufReader::new(stream);
            while let Ok(Some(request)) = read_message::<_, Request>(&mut reader) {
                let reply = match request {
                    Request::Hello { .. } => Reply::Hello { version },
                    Request::ListProfiles => Reply::Profiles { names: vec!["default".into()] },
                    _ => Reply::Error { message: "unexpected".into() },
                };
                write_message(reader.get_mut(), &reply).unwrap();
                reader.get_mut().flush().unwrap();
            }
        })
    }

    #[test]
    fn handshakes_and_requests() {
        let server = fake_daemon("k100-ipc-test-ok", PROTOCOL_VERSION);
        let mut client = Client::connect_to("k100-ipc-test-ok".to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
        assert_eq!(client.request(&Request::ListProfiles).unwrap(), Reply::Profiles { names: vec!["default".into()] });
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn refuses_other_protocol_versions() {
        let server = fake_daemon("k100-ipc-test-old", PROTOCOL_VERSION + 1);
        let error = Client::connect_to("k100-ipc-test-old".to_ns_name::<GenericNamespaced>().unwrap()).unwrap_err();
        assert!(matches!(error, ClientError::VersionMismatch { daemon } if daemon == PROTOCOL_VERSION + 1));
        server.join().unwrap();
    }
}
