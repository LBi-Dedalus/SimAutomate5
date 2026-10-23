use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

mod auto_reply;
mod config;
mod sessions;

use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};

use super::*;
use crate::models::{ConnectionStatus, MessagePayload, MessageType, StatusPayload};
use crate::reporting::{ConnectionReporterFactory, EventSink, NullLogger};

#[derive(Clone, Debug)]
enum Event {
    Status(StatusPayload),
    Message(MessagePayload),
}

#[derive(Default)]
struct TestSink {
    events: StdMutex<Vec<Event>>,
}

impl EventSink for TestSink {
    fn status(&self, payload: StatusPayload) -> Result<(), String> {
        self.events.lock().unwrap().push(Event::Status(payload));
        Ok(())
    }
    fn message(&self, payload: MessagePayload) -> Result<(), String> {
        self.events.lock().unwrap().push(Event::Message(payload));
        Ok(())
    }
    fn notify(&self, _title: &str, _body: &str) -> Result<(), String> {
        Err("no notifications in tests".to_string())
    }
}

impl TestSink {
    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    fn statuses(&self, session: &str) -> Vec<(u64, ConnectionStatus)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Status(s) if s.session_id == session => Some((s.attempt, s.status)),
                _ => None,
            })
            .collect()
    }

    fn has_status(&self, session: &str, attempt: u64, status: ConnectionStatus) -> bool {
        self.statuses(session).contains(&(attempt, status))
    }

    fn received(&self, session: &str) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Message(m)
                    if m.session_id == session && matches!(m.msg_type, MessageType::Received) =>
                {
                    Some(m.content)
                }
                _ => None,
            })
            .collect()
    }
}

fn setup() -> (Arc<TestSink>, ConnectionManager) {
    let sink = Arc::new(TestSink::default());
    let factory = ConnectionReporterFactory::new(sink.clone(), Arc::new(NullLogger));
    (sink, ConnectionManager::new(factory))
}

fn client(port: u16) -> ConnectRequest {
    ConnectRequest::ClientConnectRequest {
        ip: "127.0.0.1".to_string(),
        port,
    }
}

fn server(port: u16) -> ConnectRequest {
    ConnectRequest::ServerStartRequest { port }
}

fn msg(text: &str) -> SendRequest {
    SendRequest {
        message: text.to_string(),
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(20)).await;
    }
}

async fn peer() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Starts a client session against a fresh local peer and returns the peer side.
async fn connected_client(sink: &TestSink, state: &ConnectionManager, session: &str) -> TcpStream {
    let (listener, port) = peer().await;
    connect(state, session, 1, client(port)).await.unwrap();
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("peer accept timed out")
        .unwrap();
    wait_for("client connected", || {
        sink.has_status(session, 1, ConnectionStatus::Connected)
    })
    .await;
    stream
}

async fn read_until(stream: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
    let mut received = Vec::new();
    let mut buffer = [0u8; 256];
    loop {
        let len = timeout(Duration::from_secs(5), stream.read(&mut buffer))
            .await
            .expect("timed out waiting for bytes")
            .unwrap();
        assert!(len > 0, "peer closed while waiting for data");
        received.extend_from_slice(&buffer[..len]);
        if received.windows(needle.len()).any(|w| w == needle) {
            return received;
        }
    }
}

async fn expect_silence(stream: &mut TcpStream) {
    let mut buffer = [0u8; 64];
    let result = timeout(Duration::from_millis(500), stream.read(&mut buffer)).await;
    assert!(result.is_err(), "unexpected data: {result:?}");
}

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::auto_reply::{AutoReplyAction, AutoReplyTrigger, RawAutoReplyConfig, RawAutoReplyRule};
use crate::config_store;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Isolated config directory; only that exact directory is removed on drop.
struct TestConfig {
    dir: PathBuf,
    path: PathBuf,
    store: ConfigStore,
}

impl TestConfig {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "simautomate-autoreply-test-{}-{}",
            std::process::id(),
            DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = config_store::config_path(&dir);
        Self {
            store: ConfigStore::new(path.clone()),
            path,
            dir,
        }
    }
}

impl Drop for TestConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rule(
    id: &str,
    trigger: AutoReplyTrigger,
    action: AutoReplyAction,
    delay_ms: u32,
) -> RawAutoReplyRule {
    RawAutoReplyRule {
        id: id.to_string(),
        name: format!("Rule {id}"),
        enabled: true,
        trigger,
        condition: None,
        action,
        delay_ms,
    }
}

fn hl7_trigger(pattern: &str) -> AutoReplyTrigger {
    AutoReplyTrigger::Hl7 {
        message_type: pattern.to_string(),
    }
}

fn literal(text: &str) -> AutoReplyAction {
    AutoReplyAction::Literal {
        text: text.to_string(),
    }
}

fn template_action(id: &str) -> AutoReplyAction {
    AutoReplyAction::Template {
        template_id: id.into(),
    }
}

fn auto_reply_config(enabled: bool, rules: Vec<RawAutoReplyRule>) -> AutoReplyConfig {
    RawAutoReplyConfig { enabled, rules }.parse().unwrap()
}

async fn apply(
    cfg: &TestConfig,
    state: &ConnectionManager,
    enabled: bool,
    rules: Vec<RawAutoReplyRule>,
) {
    save_auto_reply(&cfg.store, state, &auto_reply_config(enabled, rules))
        .await
        .unwrap();
}

fn hl7_frame(msh9: &str, id: &str, extra: &str) -> Vec<u8> {
    let mut frame = vec![0x0b];
    frame.extend_from_slice(
        format!("MSH|^~\\&|S|F|R|F|20260101||{msh9}|{id}|P|2.4\r{extra}").as_bytes(),
    );
    frame.extend_from_slice(&[0x1c, 0x0d]);
    frame
}

const ASTM_FRAME: &[u8] = b"\x021H|\\^&|||x\r\x03D1\r\n";

/// Everything the peer receives during `millis`.
async fn collect(stream: &mut TcpStream, millis: u64) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_millis(millis);
    let mut received = Vec::new();
    let mut buffer = [0u8; 512];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match timeout(remaining, stream.read(&mut buffer)).await {
            Ok(Ok(len)) if len > 0 => received.extend_from_slice(&buffer[..len]),
            _ => return received,
        }
    }
}

fn system_errors(sink: &TestSink, session: &str) -> Vec<String> {
    sink.events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Message(m)
                if m.session_id == session && matches!(m.msg_type, MessageType::SystemError) =>
            {
                Some(m.content)
            }
            _ => None,
        })
        .collect()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}
