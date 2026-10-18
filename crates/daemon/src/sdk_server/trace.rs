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

pub struct Trace {
    path: PathBuf,
    file: Option<File>,
    failed: bool,
}

impl Trace {
    pub fn new(path: PathBuf) -> Self {
        Self { path, file: None, failed: false }
    }

    pub fn record(&mut self, client: ClientId, exe: &str, direction: Direction, payload: &[u8]) {
        if self.failed {
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
            self.failed = true;
            self.file = None;
        }
    }

    pub fn close(&mut self) {
        self.file = None;
        self.failed = false;
    }

    fn append(&mut self, line: &Value) -> io::Result<()> {
        let file = match &mut self.file {
            Some(file) => file,
            None => self.file.insert(self.open()?),
        };
        writeln!(file, "{line}")
    }

    fn open(&self) -> io::Result<File> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        OpenOptions::new().create(true).append(true).open(&self.path)
    }
}

fn message(payload: &[u8]) -> Value {
    serde_json::from_slice(payload).unwrap_or_else(|_| json!({"bytes": payload.len()}))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}
