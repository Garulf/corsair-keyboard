use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sdk::ClientId;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
}

pub const TRACE_LIMIT: u64 = 16 * 1024 * 1024;

pub struct Trace {
    path: PathBuf,
    limit: u64,
    file: Option<File>,
    written: u64,
    stopped: bool,
}

impl Trace {
    pub fn new(path: PathBuf) -> Self {
        Self::with_limit(path, TRACE_LIMIT)
    }

    pub fn with_limit(path: PathBuf, limit: u64) -> Self {
        Self { path, limit, file: None, written: 0, stopped: false }
    }

    pub fn record(&mut self, client: ClientId, exe: &str, direction: Direction, payload: &[u8]) {
        if self.stopped {
            return;
        }
        let line = json!({
            "at_ms": now_ms(),
            "client": client,
            "direction": direction.label(),
            "exe": exe,
            "message": message(payload),
        });
        if let Err(error) = self.append(&line) {
            eprintln!("k100d: could not write the SDK trace {}: {error}", self.path.display());
            self.stopped = true;
            self.file = None;
        }
    }

    pub fn close(&mut self) {
        self.file = None;
        self.stopped = false;
    }

    fn append(&mut self, line: &Value) -> io::Result<()> {
        let file = match &mut self.file {
            Some(file) => file,
            None => {
                let file = self.open()?;
                self.written = file.metadata()?.len();
                self.file.insert(file)
            }
        };
        if self.written >= self.limit {
            eprintln!("k100d: the SDK trace {} reached its size limit; tracing stopped", self.path.display());
            let notice =
                json!({ "at_ms": now_ms(), "notice": "the SDK trace reached its size limit; tracing stopped" });
            file.write_all(&json_line(&notice))?;
            self.stopped = true;
            self.file = None;
            return Ok(());
        }
        let bytes = json_line(line);
        file.write_all(&bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn open(&self) -> io::Result<File> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        OpenOptions::new().create(true).append(true).open(&self.path)
    }
}

fn json_line(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("a JSON value always serializes");
    bytes.push(b'\n');
    bytes
}

fn message(payload: &[u8]) -> Value {
    serde_json::from_slice(payload).unwrap_or_else(|_| json!({"bytes": payload.len()}))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    const PAYLOAD: &[u8] = br#"{"method":0,"code":-1,"params":{"pid":1,"clientVersion":"4.0.84"}}"#;

    fn lines(path: &std::path::Path) -> Vec<Value> {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.ends_with('\n'));
        text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    #[test]
    fn tracing_stops_with_a_notice_once_the_file_reaches_its_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("logs").join("sdk-trace.jsonl");
        let mut trace = Trace::with_limit(path.clone(), 1000);
        for _ in 0..50 {
            trace.record(1, "game.exe", Direction::In, PAYLOAD);
        }
        let written = lines(&path);
        let (notice, records) = written.split_last().unwrap();
        assert!(notice["notice"].as_str().unwrap().contains("trace"), "{notice}");
        assert!(notice.get("message").is_none());
        assert!(records.iter().all(|line| line["message"]["method"] == 0));
        let size = std::fs::metadata(&path).unwrap().len();
        assert!((1000..1300).contains(&size), "{size}");
        trace.record(1, "game.exe", Direction::Out, PAYLOAD);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size, "nothing is written after the notice");
    }

    #[test]
    fn the_default_limit_is_16_mib() {
        assert_eq!(TRACE_LIMIT, 16 * 1024 * 1024);
        assert_eq!(Trace::new(PathBuf::from("trace.jsonl")).limit, TRACE_LIMIT);
    }
}
