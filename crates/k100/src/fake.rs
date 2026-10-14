use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bragi::REPORT_LEN;

use crate::transport::{InputSource, Transport, TransportError};

type Responder = Box<dyn FnMut(&[u8]) -> Vec<Vec<u8>> + Send>;

pub struct FakeTransport {
    log: Arc<Mutex<Vec<Vec<u8>>>>,
    pending: VecDeque<Vec<u8>>,
    respond: Responder,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::with_responder(|request| vec![ok_reply(request)])
    }

    pub fn with_responder(respond: impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> Self {
        Self { log: Arc::default(), pending: VecDeque::new(), respond: Box::new(respond) }
    }

    pub fn log(&self) -> Arc<Mutex<Vec<Vec<u8>>>> {
        Arc::clone(&self.log)
    }

    pub fn push_unsolicited(&mut self, report: Vec<u8>) {
        self.pending.push_back(report);
    }
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for FakeTransport {
    fn write(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), TransportError> {
        self.log.lock().unwrap().push(report.to_vec());
        let replies = (self.respond)(report);
        self.pending.extend(replies);
        Ok(())
    }

    fn read(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, TransportError> {
        Ok(self.pending.pop_front())
    }
}

pub struct FakeInput {
    reports: VecDeque<Vec<u8>>,
}

impl FakeInput {
    pub fn new(reports: Vec<Vec<u8>>) -> Self {
        Self { reports: reports.into() }
    }
}

impl InputSource for FakeInput {
    fn read(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, TransportError> {
        Ok(self.reports.pop_front())
    }
}

pub fn status_reply(request: &[u8], status: u8) -> Vec<u8> {
    vec![0x00, request[1], status]
}

pub fn ok_reply(request: &[u8]) -> Vec<u8> {
    status_reply(request, 0)
}

pub fn value_reply(request: &[u8], value: u32) -> Vec<u8> {
    let mut reply = ok_reply(request);
    reply.extend_from_slice(&value.to_le_bytes());
    reply
}
