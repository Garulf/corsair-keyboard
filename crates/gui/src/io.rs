use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use ipc::{Client, ClientError, OnboardRequest, Reply, Request};

use crate::model::Inbox;

pub const RETRY_INTERVAL: Duration = Duration::from_secs(1);

pub type Connector = Arc<dyn Fn() -> Result<Client, ClientError> + Send + Sync>;
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub struct Backend {
    requests: Sender<Request>,
    inbox: Receiver<Inbox>,
    injector: Sender<Inbox>,
}

impl Backend {
    pub fn spawn(connector: Connector, wake: Waker, retry: Duration) -> Self {
        let (injector, inbox) = mpsc::channel();
        let (requests, request_rx) = mpsc::channel();
        spawn_worker(connector.clone(), request_rx, injector.clone(), wake.clone());
        spawn_events(connector, injector.clone(), wake, retry);
        Self { requests, inbox, injector }
    }

    pub fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }

    pub fn drain(&self) -> Vec<Inbox> {
        self.inbox.try_iter().collect()
    }

    pub fn injector(&self) -> Sender<Inbox> {
        self.injector.clone()
    }
}

fn emit(inbox: &Sender<Inbox>, wake: &Waker, message: Inbox) -> bool {
    let delivered = inbox.send(message).is_ok();
    wake();
    delivered
}

fn attempt(connector: &Connector, cached: &mut Option<Client>, request: &Request) -> Result<Reply, ClientError> {
    let mut live = match cached.take() {
        Some(existing) => existing,
        None => connector()?,
    };
    let reply = live.request(request)?;
    *cached = Some(live);
    Ok(reply)
}

fn retry_is_safe(request: &Request) -> bool {
    match request {
        Request::Onboard { request } => *request == OnboardRequest::List,
        _ => true,
    }
}

fn exchange(connector: &Connector, cached: &mut Option<Client>, request: &Request) -> Result<Reply, ClientError> {
    let reused = cached.is_some();
    match attempt(connector, cached, request) {
        Err(ClientError::Io(_) | ClientError::Closed) if reused && retry_is_safe(request) => {
            attempt(connector, cached, request)
        }
        outcome => outcome,
    }
}

fn spawn_worker(connector: Connector, requests: Receiver<Request>, inbox: Sender<Inbox>, wake: Waker) {
    thread::spawn(move || {
        let mut cached: Option<Client> = None;
        for request in requests {
            let message = match exchange(&connector, &mut cached, &request) {
                Ok(reply) => Inbox::reply(request, reply),
                Err(failure) => Inbox::request_failed(request, failure.to_string()),
            };
            if !emit(&inbox, &wake, message) {
                return;
            }
        }
    });
}

fn subscribe(connector: &Connector) -> Result<Client, ClientError> {
    let mut client = connector()?;
    match client.request(&Request::SubscribeFrames)? {
        Reply::Done => Ok(client),
        other => Err(ClientError::Unexpected(format!("{other:?}"))),
    }
}

fn spawn_events(connector: Connector, inbox: Sender<Inbox>, wake: Waker, retry: Duration) {
    thread::spawn(move || {
        loop {
            let reason = match subscribe(&connector) {
                Ok(mut client) => {
                    if !emit(&inbox, &wake, Inbox::Connected) {
                        return;
                    }
                    loop {
                        match client.next_event() {
                            Ok(Some(event)) => {
                                if !emit(&inbox, &wake, Inbox::Event(event)) {
                                    return;
                                }
                            }
                            Ok(None) => break "k100d closed the connection".to_owned(),
                            Err(failure) => break failure.to_string(),
                        }
                    }
                }
                Err(failure) => failure.to_string(),
            };
            if !emit(&inbox, &wake, Inbox::Disconnected(reason)) {
                return;
            }
            thread::sleep(retry);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream, prelude::*};
    use ipc::{Event, PROTOCOL_VERSION, Reply, read_message, write_message};

    use super::*;
    use crate::testing::state;

    fn serve(stream: Stream, saves: &AtomicUsize) {
        let mut reader = BufReader::new(stream);
        while let Ok(Some(request)) = read_message::<_, Request>(&mut reader) {
            let reply = match request {
                Request::Hello { .. } => Reply::Hello { version: PROTOCOL_VERSION },
                Request::GetState => Reply::State { state: state("default", &["default"]) },
                Request::SubscribeFrames => {
                    write_message(reader.get_mut(), &Reply::Done).unwrap();
                    let event = Event::ProfileChanged { name: "games".into() };
                    write_message(reader.get_mut(), &Reply::Event { event }).unwrap();
                    std::thread::sleep(Duration::from_secs(5));
                    return;
                }
                Request::Release => {
                    write_message(reader.get_mut(), &Reply::Done).unwrap();
                    return;
                }
                Request::Onboard { request: OnboardRequest::Save { .. } } => {
                    saves.fetch_add(1, Ordering::SeqCst);
                    return;
                }
                _ => Reply::Done,
            };
            write_message(reader.get_mut(), &reply).unwrap();
        }
    }

    fn fake_daemon(name: &'static str) -> Arc<AtomicUsize> {
        let listener =
            ListenerOptions::new().name(name.to_ns_name::<GenericNamespaced>().unwrap()).create_sync().unwrap();
        let saves = Arc::new(AtomicUsize::new(0));
        let counted = saves.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                let counted = counted.clone();
                std::thread::spawn(move || serve(stream, &counted));
            }
        });
        saves
    }

    fn connector_to(name: &'static str) -> Connector {
        Arc::new(move || Client::connect_to(name.to_ns_name::<GenericNamespaced>().unwrap()))
    }

    fn collect(backend: &Backend, done: impl Fn(&[Inbox]) -> bool) -> Vec<Inbox> {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !done(&seen) {
            seen.extend(backend.drain());
            std::thread::sleep(Duration::from_millis(10));
        }
        seen
    }

    #[test]
    fn reports_connected_then_events_and_answers_requests() {
        fake_daemon("k100-gui-io-ok");
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let wake: Waker = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let backend = Backend::spawn(connector_to("k100-gui-io-ok"), wake, Duration::from_millis(50));
        backend.send(Request::GetState);
        let seen = collect(&backend, |seen| {
            seen.contains(&Inbox::Connected)
                && seen.iter().any(|m| matches!(m, Inbox::Event(Event::ProfileChanged { .. })))
                && seen.iter().any(|m| matches!(m.as_reply(), Some((Request::GetState, Reply::State { .. }))))
        });
        assert!(seen.contains(&Inbox::Connected), "{seen:?}");
        assert!(seen.iter().any(|m| matches!(m, Inbox::Event(Event::ProfileChanged { name }) if name == "games")));
        assert!(seen.iter().any(|m| matches!(m.as_reply(), Some((Request::GetState, Reply::State { .. })))));
        assert!(wakes.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn an_unreachable_daemon_reports_disconnected_and_fails_requests_cleanly() {
        let connector: Connector = Arc::new(|| Err(ClientError::Closed));
        let backend = Backend::spawn(connector, Arc::new(|| {}), Duration::from_millis(20));
        backend.send(Request::GetState);
        let failed = |m: &Inbox| matches!(m, Inbox::RequestFailed(request, _) if **request == Request::GetState);
        let seen = collect(&backend, |seen| {
            seen.iter().filter(|m| matches!(m, Inbox::Disconnected(_))).count() >= 2 && seen.iter().any(failed)
        });
        assert!(seen.iter().filter(|m| matches!(m, Inbox::Disconnected(_))).count() >= 2, "retries: {seen:?}");
        assert!(seen.iter().any(failed), "{seen:?}");
        assert!(!seen.iter().any(|m| m.as_reply().is_some()), "a lost request is not a daemon reply: {seen:?}");
    }

    #[test]
    fn it_reconnects_by_itself_when_the_daemon_shows_up_late() {
        fake_daemon("k100-gui-io-late");
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        let connector: Connector = Arc::new(move || {
            if counter.fetch_add(1, Ordering::SeqCst) < 2 {
                Err(ClientError::Closed)
            } else {
                Client::connect_to("k100-gui-io-late".to_ns_name::<GenericNamespaced>().unwrap())
            }
        });
        let backend = Backend::spawn(connector, Arc::new(|| {}), Duration::from_millis(30));
        let seen = collect(&backend, |seen| seen.contains(&Inbox::Connected));
        assert!(seen.iter().any(|m| matches!(m, Inbox::Disconnected(_))));
        assert!(seen.contains(&Inbox::Connected), "{seen:?}");
    }

    #[test]
    fn the_request_worker_reconnects_once_after_the_daemon_restarts() {
        fake_daemon("k100-gui-io-restart");
        let backend = Backend::spawn(connector_to("k100-gui-io-restart"), Arc::new(|| {}), Duration::from_secs(60));
        backend.send(Request::Release);
        let first = collect(&backend, |seen| seen.iter().any(|m| m.as_reply().is_some()));
        assert!(first.iter().any(|m| matches!(m.as_reply(), Some((Request::Release, Reply::Done)))), "{first:?}");
        backend.send(Request::GetState);
        let second =
            collect(&backend, |seen| seen.iter().any(|m| matches!(m.as_reply(), Some((Request::GetState, _)))));
        assert!(
            second.iter().any(|m| matches!(m.as_reply(), Some((Request::GetState, Reply::State { .. })))),
            "{second:?}"
        );
    }

    #[test]
    fn a_save_cut_off_by_a_dropped_connection_is_sent_once_and_reported() {
        let saves = fake_daemon("k100-gui-io-save");
        let backend = Backend::spawn(connector_to("k100-gui-io-save"), Arc::new(|| {}), Duration::from_secs(60));
        backend.send(Request::GetState);
        collect(&backend, |seen| seen.iter().any(|m| m.as_reply().is_some()));
        let save = Request::Onboard { request: OnboardRequest::Save { profile: "games".into(), slot: None } };
        backend.send(save.clone());
        let seen = collect(&backend, |seen| seen.iter().any(|m| matches!(m, Inbox::RequestFailed(..))));
        assert!(seen.iter().any(|m| matches!(m, Inbox::RequestFailed(request, _) if **request == save)), "{seen:?}");
        assert_eq!(saves.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn only_a_listing_may_be_sent_again_after_a_dropped_connection() {
        let onboard = |request| Request::Onboard { request };
        assert!(retry_is_safe(&onboard(OnboardRequest::List)));
        assert!(retry_is_safe(&Request::GetState));
        assert!(!retry_is_safe(&onboard(OnboardRequest::Save { profile: "a".into(), slot: None })));
        assert!(!retry_is_safe(&onboard(OnboardRequest::Delete { slot: 1 })));
        assert!(!retry_is_safe(&onboard(OnboardRequest::Activate { slot: 1 })));
        assert!(!retry_is_safe(&onboard(OnboardRequest::Backup { to: None })));
        assert!(!retry_is_safe(&onboard(OnboardRequest::Restore { from: "/b".into() })));
    }
}
