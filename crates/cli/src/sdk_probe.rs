use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sdk::{Direction, ProbeOptions, ProbePipe, Transcript, differences, run_probe};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct ProbeArgs {
    pub out: Option<PathBuf>,
    pub write: bool,
    pub listen_seconds: u64,
    pub pipe: String,
}

pub fn probe(args: ProbeArgs) -> Result<()> {
    let mut pipe = open_pipe(&args.pipe)?;
    let listen_seconds = args.listen_seconds;
    let options = ProbeOptions {
        write: args.write,
        listen: Duration::from_secs(listen_seconds),
        pid: std::process::id(),
        ..ProbeOptions::default()
    };
    let mut transcript = run_probe(pipe.as_mut(), &options, &mut || {
        println!("listening for {listen_seconds} s: press G1, then G6");
    });
    transcript.pipe = pipe_path(&args.pipe);
    let path = args.out.unwrap_or_else(|| default_out_name(unix_seconds()));
    std::fs::write(&path, serde_json::to_string_pretty(&transcript)?)?;
    println!("{}", path.display());
    println!("{}", summary(&transcript));
    Ok(())
}

pub fn compare(expected: &Path, actual: &Path) -> Result<Vec<String>> {
    Ok(differences(&load(expected)?, &load(actual)?))
}

pub fn print_comparison(expected: &Path, actual: &Path) -> Result<()> {
    let lines = compare(expected, actual)?;
    if lines.is_empty() {
        println!("no differences");
        return Ok(());
    }
    for line in &lines {
        println!("{line}");
    }
    std::process::exit(1);
}

fn load(path: &Path) -> Result<Transcript> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()).into())
}

fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\{name}")
}

fn default_out_name(unix_seconds: u64) -> PathBuf {
    PathBuf::from(format!("k100-sdk-probe-{unix_seconds}.json"))
}

fn unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

fn summary(transcript: &Transcript) -> String {
    let count = |direction| transcript.entries.iter().filter(|entry| entry.direction == direction).count();
    format!(
        "sent {}, received {}, unanswered {}",
        count(Direction::Sent),
        count(Direction::Received),
        transcript.unanswered().len()
    )
}

#[cfg(not(windows))]
fn open_pipe(_name: &str) -> Result<Box<dyn ProbePipe>> {
    Err("sdk-probe needs Windows; the iCUE SDK pipe exists only there".into())
}

#[cfg(windows)]
fn open_pipe(name: &str) -> Result<Box<dyn ProbePipe>> {
    Ok(Box::new(named_pipe::open(name)?))
}

#[cfg(any(windows, test))]
mod polling {
    use std::io::{self, Read, Write};
    use std::time::{Duration, Instant};

    use sdk::ProbePipe;

    pub struct Peeked {
        pub copied: usize,
        pub total: usize,
    }

    pub trait PeekStream: Read + Write {
        fn peek(&mut self, buffer: &mut [u8]) -> io::Result<Peeked>;
    }

    const LENGTH_PREFIX: usize = 4;

    pub struct PollingPipe<S> {
        pub stream: S,
        pub poll_interval: Duration,
    }

    impl<S: PeekStream> ProbePipe for PollingPipe<S> {
        fn send(&mut self, payload: &[u8]) -> io::Result<()> {
            self.stream.write_all(&sdk::encode_frame(payload))?;
            self.stream.flush()
        }

        fn receive(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(payload) = self.take_frame()? {
                    return Ok(Some(payload));
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                std::thread::sleep(remaining.min(self.poll_interval));
            }
        }
    }

    impl<S: PeekStream> PollingPipe<S> {
        fn take_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
            let mut prefix = [0u8; LENGTH_PREFIX];
            let peeked = self.stream.peek(&mut prefix)?;
            if peeked.copied < LENGTH_PREFIX {
                return Ok(None);
            }
            let length = u32::from_be_bytes(prefix) as usize;
            if length > sdk::MAX_FRAME {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("frame of {length} bytes is over the limit"),
                ));
            }
            if peeked.total < LENGTH_PREFIX + length {
                return Ok(None);
            }
            let mut frame = vec![0u8; LENGTH_PREFIX + length];
            self.stream.read_exact(&mut frame)?;
            Ok(Some(frame.split_off(LENGTH_PREFIX)))
        }
    }
}

#[cfg(windows)]
mod named_pipe {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::time::Duration;

    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    use super::polling::{PeekStream, Peeked, PollingPipe};
    use super::{Result, pipe_path};

    const POLL_INTERVAL: Duration = Duration::from_millis(10);

    pub struct NamedPipe(File);

    impl Read for NamedPipe {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.read(buffer)
        }
    }

    impl Write for NamedPipe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    impl PeekStream for NamedPipe {
        fn peek(&mut self, buffer: &mut [u8]) -> io::Result<Peeked> {
            let (mut copied, mut total) = (0u32, 0u32);
            let size = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
            let peeked = unsafe {
                PeekNamedPipe(
                    self.0.as_raw_handle(),
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut copied,
                    &mut total,
                    std::ptr::null_mut(),
                )
            };
            if peeked == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Peeked { copied: copied as usize, total: total as usize })
        }
    }

    pub fn open(name: &str) -> Result<PollingPipe<NamedPipe>> {
        let path = pipe_path(name);
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => Ok(PollingPipe { stream: NamedPipe(file), poll_interval: POLL_INTERVAL }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(format!("nothing serves {path}: start k100d or iCUE with its SDK turned on").into())
            }
            Err(error) => Err(format!("cannot open {path}: {error}").into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::polling::{PeekStream, Peeked, PollingPipe};
    use super::*;
    use sdk::{Direction, Entry};
    use serde_json::json;
    use std::cell::Cell;
    use std::io::{self, Read, Write};

    struct Trickle {
        incoming: Vec<u8>,
        released: Cell<usize>,
        read: usize,
        written: Vec<u8>,
    }

    impl Trickle {
        fn with(incoming: Vec<u8>) -> Self {
            Self { incoming, released: Cell::new(0), read: 0, written: Vec::new() }
        }

        fn available(&self) -> &[u8] {
            &self.incoming[self.read..self.released.get().min(self.incoming.len())]
        }
    }

    impl Read for Trickle {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.available().len().min(buffer.len());
            buffer[..count].copy_from_slice(&self.available()[..count]);
            self.read += count;
            Ok(count)
        }
    }

    impl Write for Trickle {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl PeekStream for Trickle {
        fn peek(&mut self, buffer: &mut [u8]) -> io::Result<Peeked> {
            self.released.set(self.released.get() + 1);
            let available = self.available();
            let copied = available.len().min(buffer.len());
            buffer[..copied].copy_from_slice(&available[..copied]);
            Ok(Peeked { copied, total: available.len() })
        }
    }

    fn pipe_over(stream: Trickle) -> PollingPipe<Trickle> {
        PollingPipe { stream, poll_interval: Duration::from_millis(1) }
    }

    #[test]
    fn a_frame_that_arrives_a_byte_at_a_time_is_read_whole() {
        let mut pipe = pipe_over(Trickle::with(sdk::encode_frame(b"{\"method\":0}")));
        assert_eq!(pipe.receive(Duration::from_secs(5)).unwrap(), Some(b"{\"method\":0}".to_vec()));
    }

    #[test]
    fn two_frames_in_the_pipe_come_out_one_at_a_time() {
        let both = [sdk::encode_frame(b"one"), sdk::encode_frame(b"two")].concat();
        let mut pipe = pipe_over(Trickle::with(both));
        assert_eq!(pipe.receive(Duration::from_secs(5)).unwrap(), Some(b"one".to_vec()));
        assert_eq!(pipe.receive(Duration::from_secs(5)).unwrap(), Some(b"two".to_vec()));
    }

    #[test]
    fn an_empty_pipe_times_out_with_none() {
        let mut pipe = pipe_over(Trickle::with(Vec::new()));
        assert_eq!(pipe.receive(Duration::from_millis(5)).unwrap(), None);
    }

    #[test]
    fn a_half_frame_times_out_with_none_and_is_not_consumed() {
        let mut frame = sdk::encode_frame(b"abcdef");
        frame.truncate(6);
        let mut pipe = pipe_over(Trickle::with(frame));
        assert_eq!(pipe.receive(Duration::from_millis(5)).unwrap(), None);
        assert_eq!(pipe.stream.read, 0);
    }

    #[test]
    fn a_length_over_the_limit_is_an_error() {
        let mut pipe = pipe_over(Trickle::with(vec![0xFF; 4]));
        let error = pipe.receive(Duration::from_secs(1)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn sending_writes_the_length_prefix_and_the_payload_in_one_write() {
        let mut pipe = pipe_over(Trickle::with(Vec::new()));
        pipe.send(b"{}").unwrap();
        assert_eq!(pipe.stream.written, [0, 0, 0, 2, b'{', b'}']);
    }

    #[test]
    fn compare_lists_differences_between_two_saved_transcripts() {
        let folder = tempfile::tempdir().unwrap();
        let base = include_str!("../../sdk/fixtures/handshake.json");
        let (a, b) = (folder.path().join("a.json"), folder.path().join("b.json"));
        std::fs::write(&a, base).unwrap();
        std::fs::write(&b, base.replace("K70 MAX", "K100 RGB")).unwrap();
        assert!(compare(&a, &a).unwrap().is_empty());
        assert_eq!(compare(&a, &b).unwrap().len(), 1);
    }

    #[test]
    fn compare_names_the_file_it_cannot_read() {
        let folder = tempfile::tempdir().unwrap();
        let missing = folder.path().join("missing.json");
        let error = compare(&missing, &missing).unwrap_err().to_string();
        assert!(error.contains("missing.json"), "{error}");
    }

    #[test]
    fn the_summary_counts_sent_received_and_unanswered_requests() {
        let entry = |direction, message| Entry { at_ms: 0, direction, message };
        let transcript = sdk::Transcript {
            tool: String::new(),
            pipe: String::new(),
            mode: "read-only".into(),
            entries: vec![
                entry(Direction::Sent, json!({"method": 0, "code": -1})),
                entry(Direction::Received, json!({"method": 0, "code": -1})),
                entry(Direction::Sent, json!({"method": 1, "code": 1})),
                entry(Direction::Note, json!({"timeout": {"method": 1, "code": 1}})),
                entry(Direction::Received, json!({"method": 17})),
            ],
        };
        assert_eq!(summary(&transcript), "sent 2, received 2, unanswered 1");
    }

    #[test]
    fn the_default_file_name_carries_the_unix_time() {
        assert_eq!(default_out_name(1_790_000_000), PathBuf::from("k100-sdk-probe-1790000000.json"));
    }

    #[cfg(not(windows))]
    #[test]
    fn sdk_probe_explains_it_needs_windows() {
        let args = ProbeArgs { out: None, write: false, listen_seconds: 0, pipe: "iCUESDKv4".into() };
        let error = probe(args).unwrap_err().to_string();
        assert!(error.contains("needs Windows"), "{error}");
    }
}
