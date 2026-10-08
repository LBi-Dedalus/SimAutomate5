//! Session level operations behind the Tauri commands. They are plain functions over the shared
//! application state so that the exact same code is exercised by the tests with real sockets.
//!
//! The global state lock is only ever held for quick, non-blocking registry operations: waiting
//! for a transport task to finish happens with the lock released.

//!
//! Configuration operations (templates, auto reply rules) always take the `ConfigLock` first,
//! then (briefly) the application state, then the queues: read -> validate/compile -> atomic
//! write -> apply the validated rule set to the running connections. Nothing is applied when a
//! step fails, so a failed save keeps the previous behaviour.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app_state::AppState;
use crate::auto_reply::{AutoReplyConfig, RuleSet};
use crate::config_store::{self, ConfigLock, LoadedAutoReply, LoadedTemplates, Template};
use crate::models::{ConnectRequest, SendRequest};

pub async fn connect(
    state: &Mutex<AppState>,
    session_id: &str,
    attempt: u64,
    req: ConnectRequest,
) -> Result<(), String> {
    state
        .lock()
        .await
        .connection_manager
        .connect(session_id, attempt, req)
}

/// Queues a message on the given attempt of the given session, only if it is connected.
pub async fn send(
    state: &Mutex<AppState>,
    session_id: &str,
    attempt: u64,
    payload: &SendRequest,
) -> Result<(), String> {
    let queue = state
        .lock()
        .await
        .connection_manager
        .send_target(session_id, attempt)?;
    queue.send_user_message(payload).await;
    Ok(())
}

/// Stops one session. With `remove` the session is forgotten (close), otherwise it stays
/// registered and can be reconnected (disconnect).
pub async fn stop(state: &Mutex<AppState>, session_id: &str, remove: bool) -> Result<(), String> {
    let mut ticket = state
        .lock()
        .await
        .connection_manager
        .begin_stop(session_id, remove)?;
    // The global lock is released here: other sessions keep working while this one tears down.
    ticket.run().await;
    state.lock().await.connection_manager.finish_stop(&ticket);
    Ok(())
}

async fn apply_rules(state: &Mutex<AppState>, rules: Arc<RuleSet>) {
    state
        .lock()
        .await
        .connection_manager
        .apply_rules(rules)
        .await;
}

/// Startup: loads the persisted rules and templates before any connection exists. Invalid
/// data leaves automatic replies disabled (the file is never touched) and is logged with
/// metadata only; the Rules view reports the same failure when it loads.
pub async fn init_auto_reply(lock: &ConfigLock, path: &Path, state: &Mutex<AppState>) {
    let _guard = lock.0.lock().await;
    match config_store::load_rule_set_from(path) {
        Ok(rules) => apply_rules(state, rules).await,
        Err(err) => {
            state.lock().await.emitter.only_log(
                crate::models::LogLevel::Err,
                file!(),
                line!(),
                format!("auto reply disabled, the saved configuration is invalid: {err}"),
            );
        }
    }
}

pub async fn load_templates(lock: &ConfigLock, path: &Path) -> Result<LoadedTemplates, String> {
    let _guard = lock.0.lock().await;
    config_store::load_templates_from(path)
}

pub async fn load_auto_reply(lock: &ConfigLock, path: &Path) -> Result<LoadedAutoReply, String> {
    let _guard = lock.0.lock().await;
    config_store::load_auto_reply_from(path)
}

/// Saves the templates (refused when a rule would break) and refreshes the running rules.
pub async fn save_templates(
    lock: &ConfigLock,
    path: &Path,
    state: &Mutex<AppState>,
    templates: &[Template],
) -> Result<(), String> {
    let _guard = lock.0.lock().await;
    let rules = config_store::save_templates_to(path, templates)?;
    apply_rules(state, rules).await;
    Ok(())
}

pub async fn save_auto_reply(
    lock: &ConfigLock,
    path: &Path,
    state: &Mutex<AppState>,
    config: &AutoReplyConfig,
) -> Result<(), String> {
    let _guard = lock.0.lock().await;
    let rules = config_store::save_auto_reply_to(path, config)?;
    apply_rules(state, rules).await;
    Ok(())
}

/// Persists and applies the master switch only; the saved rules are not touched.
pub async fn set_auto_reply_enabled(
    lock: &ConfigLock,
    path: &Path,
    state: &Mutex<AppState>,
    enabled: bool,
) -> Result<(), String> {
    let _guard = lock.0.lock().await;
    let rules = config_store::set_auto_reply_enabled_in(path, enabled)?;
    apply_rules(state, rules).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::{sleep, timeout};

    use super::*;
    use crate::emitter::{Emitter, EventSink};
    use crate::models::{ConnectionStatus, MessagePayload, MessageType, StatusPayload};

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
                        if m.session_id == session
                            && matches!(m.msg_type, MessageType::Received) =>
                    {
                        Some(m.content)
                    }
                    _ => None,
                })
                .collect()
        }
    }

    fn setup() -> (Arc<TestSink>, Mutex<AppState>) {
        let sink = Arc::new(TestSink::default());
        let state = Mutex::new(AppState::new(Emitter::with_sink(sink.clone())));
        (sink, state)
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
    async fn connected_client(
        sink: &TestSink,
        state: &Mutex<AppState>,
        session: &str,
    ) -> TcpStream {
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sessions_send_and_receive_independently() {
        let (sink, state) = setup();
        let mut peer_a = connected_client(&sink, &state, "a").await;
        let mut peer_b = connected_client(&sink, &state, "b").await;

        send(&state, "a", 1, &msg("hello-a")).await.unwrap();
        read_until(&mut peer_a, b"hello-a").await;
        expect_silence(&mut peer_b).await;

        send(&state, "b", 1, &msg("hello-b")).await.unwrap();
        read_until(&mut peer_b, b"hello-b").await;

        peer_a.write_all(b"from-a").await.unwrap();
        wait_for("a receives", || sink.received("a") == vec!["from-a"]).await;
        peer_b.write_all(b"from-b").await.unwrap();
        wait_for("b receives", || sink.received("b") == vec!["from-b"]).await;
        assert_eq!(sink.received("a"), vec!["from-a"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_server_session_runs_next_to_a_client_session() {
        let (sink, state) = setup();
        let mut peer_a = connected_client(&sink, &state, "a").await;

        let port = free_port();
        connect(&state, "srv", 1, server(port)).await.unwrap();
        wait_for("listening", || {
            sink.has_status("srv", 1, ConnectionStatus::Listening)
        })
        .await;
        // While listening nothing can be sent, and the other session is untouched.
        assert!(send(&state, "srv", 1, &msg("x")).await.is_err());
        let mut srv_peer = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        wait_for("server connected", || {
            sink.has_status("srv", 1, ConnectionStatus::Connected)
        })
        .await;

        send(&state, "srv", 1, &msg("from-server")).await.unwrap();
        read_until(&mut srv_peer, b"from-server").await;
        expect_silence(&mut peer_a).await;
        srv_peer.write_all(b"to-server").await.unwrap();
        wait_for("server receives", || {
            sink.received("srv") == vec!["to-server"]
        })
        .await;
        assert!(sink.received("a").is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn acknowledgements_and_waits_never_cross_sessions() {
        let (sink, state) = setup();
        let mut peer_a = connected_client(&sink, &state, "a").await;
        let mut peer_b = connected_client(&sink, &state, "b").await;

        send(&state, "a", 1, &msg("<ENQ>\nsecond-a")).await.unwrap();
        read_until(&mut peer_a, &[0x05]).await;
        // A now waits for an ACK: its second line is held back.
        expect_silence(&mut peer_a).await;

        // B is not blocked by A's pending ACK.
        send(&state, "b", 1, &msg("<ENQ>\nsecond-b")).await.unwrap();
        read_until(&mut peer_b, &[0x05]).await;
        expect_silence(&mut peer_b).await;

        // An ACK received by B releases B only.
        peer_b.write_all(&[0x06]).await.unwrap();
        read_until(&mut peer_b, b"second-b").await;
        expect_silence(&mut peer_a).await;

        peer_a.write_all(&[0x06]).await.unwrap();
        read_until(&mut peer_a, b"second-a").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_bind_conflict_only_fails_the_second_session() {
        let (sink, state) = setup();
        let port = free_port();
        connect(&state, "first", 1, server(port)).await.unwrap();
        wait_for("listening", || {
            sink.has_status("first", 1, ConnectionStatus::Listening)
        })
        .await;

        connect(&state, "second", 1, server(port)).await.unwrap();
        wait_for("second fails", || {
            sink.has_status("second", 1, ConnectionStatus::Error)
        })
        .await;

        assert!(!sink.has_status("first", 1, ConnectionStatus::Error));
        let mut peer = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        wait_for("first connected", || {
            sink.has_status("first", 1, ConnectionStatus::Connected)
        })
        .await;
        send(&state, "first", 1, &msg("alive")).await.unwrap();
        read_until(&mut peer, b"alive").await;
        // The failed session stays registered and reports that it cannot send.
        assert!(send(&state, "second", 1, &msg("x")).await.is_err());
        // It can be retried once the port is free.
        stop(&state, "second", false).await.unwrap();
        connect(&state, "second", 2, server(port)).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn invalid_requests_are_errors_and_create_nothing() {
        let (sink, state) = setup();
        assert!(connect(&state, "", 1, server(free_port())).await.is_err());
        assert!(connect(&state, "bad id", 1, server(free_port()))
            .await
            .is_err());
        assert!(connect(&state, "ok", 0, server(free_port())).await.is_err());
        assert!(connect(&state, "ok", 1, server(0)).await.is_err());
        assert!(connect(&state, "ok", 1, client(0)).await.is_err());
        // None of the rejected requests registered the session or produced events.
        assert!(stop(&state, "ok", true).await.is_err());
        assert!(sink.events().is_empty());

        assert!(send(&state, "ghost", 1, &msg("x")).await.is_err());
        assert!(stop(&state, "ghost", false).await.is_err());
        assert!(stop(&state, "ghost", true).await.is_err());
        assert!(stop(&state, "bad id", true).await.is_err());

        // A real session: wrong attempt and "already running" are rejected too.
        let port = free_port();
        connect(&state, "s", 3, server(port)).await.unwrap();
        assert!(send(&state, "s", 2, &msg("x")).await.is_err());
        assert!(connect(&state, "s", 4, server(free_port())).await.is_err());
        stop(&state, "s", false).await.unwrap();
        // Disconnected: sending fails, and an old attempt number cannot be reused.
        assert!(send(&state, "s", 3, &msg("x")).await.is_err());
        assert!(connect(&state, "s", 3, server(port)).await.is_err());
        assert!(connect(&state, "s", 2, server(port)).await.is_err());
        connect(&state, "s", 4, server(port)).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_peer_closing_only_releases_its_own_session() {
        let (sink, state) = setup();
        let peer_a = connected_client(&sink, &state, "a").await;
        let mut peer_b = connected_client(&sink, &state, "b").await;

        drop(peer_a);
        wait_for("a fails", || {
            sink.has_status("a", 1, ConnectionStatus::Error)
        })
        .await;
        assert!(send(&state, "a", 1, &msg("x")).await.is_err());

        assert!(!sink.has_status("b", 1, ConnectionStatus::Error));
        send(&state, "b", 1, &msg("still-b")).await.unwrap();
        read_until(&mut peer_b, b"still-b").await;

        // The failed session is reconnectable without any manual cleanup.
        let (listener, port) = peer().await;
        connect(&state, "a", 2, client(port)).await.unwrap();
        let (_peer_a2, _) = listener.accept().await.unwrap();
        wait_for("a reconnected", || {
            sink.has_status("a", 2, ConnectionStatus::Connected)
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stopping_a_listening_server_frees_its_port_promptly() {
        let (sink, state) = setup();
        let port = free_port();
        connect(&state, "srv", 1, server(port)).await.unwrap();
        wait_for("listening", || {
            sink.has_status("srv", 1, ConnectionStatus::Listening)
        })
        .await;

        let started = Instant::now();
        timeout(Duration::from_secs(3), stop(&state, "srv", false))
            .await
            .expect("disconnect deadlocked")
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(sink.has_status("srv", 1, ConnectionStatus::Disconnected));
        std::net::TcpListener::bind(("0.0.0.0", port)).expect("the port must be released");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_connecting_session_can_be_cancelled() {
        let (sink, state) = setup();
        // Non routable address: the attempt stays in "connecting" (or fails at once on
        // machines without a route); the cancellation must be prompt either way.
        let req = ConnectRequest::ClientConnectRequest {
            ip: "10.255.255.1".to_string(),
            port: 9,
        };
        connect(&state, "c", 1, req).await.unwrap();
        sleep(Duration::from_millis(200)).await;
        timeout(Duration::from_secs(3), stop(&state, "c", false))
            .await
            .expect("cancelling a connecting session deadlocked")
            .unwrap();
        // Depending on the network the attempt is still connecting, failed, or (behind a
        // transparent proxy) already connected: the point is the prompt, deadlock-free stop.
        let last = sink.statuses("c").last().copied().unwrap();
        assert!(matches!(
            last.1,
            ConnectionStatus::Disconnected | ConnectionStatus::Error
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reconnecting_immediately_never_mixes_attempts() {
        let (sink, state) = setup();
        let port = free_port();
        connect(&state, "s", 1, server(port)).await.unwrap();
        wait_for("listening 1", || {
            sink.has_status("s", 1, ConnectionStatus::Listening)
        })
        .await;
        stop(&state, "s", false).await.unwrap();
        let events_after_stop = sink.events().len();

        connect(&state, "s", 2, server(port)).await.unwrap();
        wait_for("listening 2", || {
            sink.has_status("s", 2, ConnectionStatus::Listening)
        })
        .await;
        let _peer = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        wait_for("connected 2", || {
            sink.has_status("s", 2, ConnectionStatus::Connected)
        })
        .await;
        sleep(Duration::from_millis(300)).await;

        let events = sink.events();
        let attempts: Vec<u64> = events
            .iter()
            .map(|e| match e {
                Event::Status(s) => s.attempt,
                Event::Message(m) => m.attempt,
            })
            .collect();
        // Everything emitted before the new attempt started belongs to the old one, and the
        // old attempt emitted nothing afterwards.
        assert!(attempts[..events_after_stop].iter().all(|a| *a == 1));
        assert!(attempts[events_after_stop..].iter().all(|a| *a == 2));
        assert_eq!(
            sink.statuses("s")
                .iter()
                .filter(|s| s.0 == 1)
                .last()
                .unwrap()
                .1,
            ConnectionStatus::Disconnected
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn closing_removes_the_session_and_other_sessions_keep_working() {
        let (sink, state) = setup();
        let mut peer_a = connected_client(&sink, &state, "a").await;
        let mut peer_b = connected_client(&sink, &state, "b").await;

        stop(&state, "a", true).await.unwrap();
        assert!(sink.has_status("a", 1, ConnectionStatus::Disconnected));
        // Peer A observes the closed socket; the session is unknown from now on.
        let mut buffer = [0u8; 8];
        assert_eq!(peer_a.read(&mut buffer).await.unwrap_or(0), 0);
        assert!(send(&state, "a", 1, &msg("x")).await.is_err());
        assert!(stop(&state, "a", true).await.is_err());

        send(&state, "b", 1, &msg("b-ok")).await.unwrap();
        read_until(&mut peer_b, b"b-ok").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stopping_one_session_does_not_block_connecting_another() {
        let (sink, state) = setup();
        let port = free_port();
        connect(&state, "a", 1, server(port)).await.unwrap();
        wait_for("listening", || {
            sink.has_status("a", 1, ConnectionStatus::Listening)
        })
        .await;
        let (_listener, peer_port) = peer().await;

        let result = timeout(Duration::from_secs(5), async {
            tokio::join!(
                stop(&state, "a", true),
                connect(&state, "b", 1, client(peer_port))
            )
        })
        .await
        .expect("deadlock between stop and connect");
        result.0.unwrap();
        result.1.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn exit_shutdown_releases_every_transport() {
        let (sink, state) = setup();
        let port = free_port();
        connect(&state, "srv", 1, server(port)).await.unwrap();
        wait_for("listening", || {
            sink.has_status("srv", 1, ConnectionStatus::Listening)
        })
        .await;
        let mut peer = connected_client(&sink, &state, "cli").await;

        state.lock().await.connection_manager.shutdown_now();

        wait_for("port released", || {
            std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
        })
        .await;
        let mut buffer = [0u8; 8];
        let closed = timeout(Duration::from_secs(5), peer.read(&mut buffer))
            .await
            .expect("peer still open");
        assert_eq!(closed.unwrap_or(0), 0);
    }

    // ── Auto reply (real loopback sockets, real config files in isolated directories) ──

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::auto_reply::{Action, Condition, Operator, Rule, Trigger};
    use crate::config_store::TemplateVariable;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Isolated config directory; only that exact directory is removed on drop.
    struct TestConfig {
        dir: PathBuf,
        path: PathBuf,
        lock: ConfigLock,
    }

    impl TestConfig {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "simautomate-autoreply-test-{}-{}",
                std::process::id(),
                DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self {
                path: config_store::config_path(&dir),
                dir,
                lock: ConfigLock(Mutex::new(())),
            }
        }
    }

    impl Drop for TestConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn rule(id: &str, trigger: Trigger, action: Action, delay_ms: u32) -> Rule {
        Rule {
            id: id.to_string(),
            name: format!("Rule {id}"),
            enabled: true,
            trigger,
            condition: None,
            action,
            delay_ms,
        }
    }

    fn hl7_trigger(pattern: &str) -> Trigger {
        Trigger::Hl7 {
            message_type: pattern.to_string(),
        }
    }

    fn literal(text: &str) -> Action {
        Action::Literal {
            text: text.to_string(),
        }
    }

    fn template_action(id: &str) -> Action {
        Action::Template {
            template_id: id.to_string(),
        }
    }

    async fn apply(cfg: &TestConfig, state: &Mutex<AppState>, enabled: bool, rules: Vec<Rule>) {
        save_auto_reply(
            &cfg.lock,
            &cfg.path,
            state,
            &AutoReplyConfig { enabled, rules },
        )
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

    const ASTM_FRAME: &[u8] = b"\x021H|\\^&|||x\r\x0342\r\n";

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
                    if m.session_id == session
                        && matches!(m.msg_type, MessageType::SystemError) =>
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn saved_rules_apply_to_current_and_future_sessions_and_off_stops_them() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        let mut old = connected_client(&sink, &state, "old").await;
        old.write_all(&[0x05]).await.unwrap();
        expect_silence(&mut old).await;

        apply(
            &cfg,
            &state,
            true,
            vec![rule("r1", Trigger::AstmEnq, literal("<ACK>"), 0)],
        )
        .await;
        old.write_all(&[0x05]).await.unwrap();
        read_until(&mut old, &[0x06]).await;

        let mut fresh = connected_client(&sink, &state, "new").await;
        fresh.write_all(&[0x05]).await.unwrap();
        read_until(&mut fresh, &[0x06]).await;

        let port = free_port();
        connect(&state, "srv", 1, server(port)).await.unwrap();
        wait_for("listening", || {
            sink.has_status("srv", 1, ConnectionStatus::Listening)
        })
        .await;
        let mut srv_peer = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        srv_peer.write_all(&[0x05]).await.unwrap();
        read_until(&mut srv_peer, &[0x06]).await;

        // The master switch alone turns every session off, without touching the rules.
        set_auto_reply_enabled(&cfg.lock, &cfg.path, &state, false)
            .await
            .unwrap();
        old.write_all(&[0x05]).await.unwrap();
        fresh.write_all(&[0x05]).await.unwrap();
        expect_silence(&mut old).await;
        expect_silence(&mut fresh).await;
        let loaded = load_auto_reply(&cfg.lock, &cfg.path).await.unwrap();
        assert!(!loaded.config.enabled && loaded.config.rules.len() == 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn first_matching_rule_wins_with_templates_and_request_control_id() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        let templates = vec![
            Template {
                id: "adr".into(),
                name: "ADR".into(),
                description: String::new(),
                payload: "<VT>MSH|^~\\&|X|Y|R|F|{{NOW}}||ADR^A19|{{CONTROL_ID}}|P|2.4<CR>MSA|AA|{{REQ_CONTROL_ID}}<CR><FS><CR>".into(),
                variables: vec![TemplateVariable { name: "REQ_CONTROL_ID".into(), default: "STALE".into() }],
            },
            Template {
                id: "nf".into(),
                name: "Not found".into(),
                description: String::new(),
                payload: "NOT-FOUND {{REQ_CONTROL_ID}}".into(),
                variables: vec![],
            },
        ];
        save_templates(&cfg.lock, &cfg.path, &state, &templates)
            .await
            .unwrap();
        let mut specific = rule("a", hl7_trigger("QRY^A19"), template_action("adr"), 0);
        specific.condition = Some(Condition {
            segment: "QRD".into(),
            field: 8,
            operator: Operator::Glob,
            value: "AAZ*".into(),
        });
        apply(
            &cfg,
            &state,
            true,
            vec![
                specific,
                rule("b", hl7_trigger("QRY^A19"), template_action("nf"), 0),
            ],
        )
        .await;

        let mut peer = connected_client(&sink, &state, "s").await;
        peer.write_all(&hl7_frame("QRY^A19", "REQ-1", "QRD|1|2|3|4|5|6|7|AAZ42\r"))
            .await
            .unwrap();
        let first = text(&read_until(&mut peer, b"\x1c\r").await);
        assert!(
            first.contains("||ADR^A19|") && first.contains("MSA|AA|REQ-1\r"),
            "{first:?}"
        );
        assert!(!first.contains("STALE"));

        peer.write_all(&hl7_frame("QRY^A19", "REQ-2", "QRD|1|2|3|4|5|6|7|OTHER\r"))
            .await
            .unwrap();
        assert_eq!(
            text(&read_until(&mut peer, b"REQ-2").await),
            "NOT-FOUND REQ-2"
        );

        // Unmatched and disabled: nothing at all.
        peer.write_all(&hl7_frame("ADT^A01", "REQ-3", ""))
            .await
            .unwrap();
        expect_silence(&mut peer).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_auto_reply_rule_silences_only_its_matches_and_stops_lower_rules() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        let mut quiet = rule("quiet", hl7_trigger("QRY^A19"), Action::None, 0);
        quiet.condition = Some(Condition {
            segment: "QRD".into(),
            field: 8,
            operator: Operator::Glob,
            value: "AAZ*".into(),
        });
        let general = rule(
            "ack",
            hl7_trigger("*"),
            Action::Hl7Ack {
                message_type: "ACK".into(),
                code: "AA".into(),
            },
            0,
        );
        apply(&cfg, &state, true, vec![quiet.clone(), general.clone()]).await;

        let mut peer = connected_client(&sink, &state, "s").await;
        let mut other = connected_client(&sink, &state, "o").await;
        peer.write_all(&hl7_frame("QRY^A19", "Q-1", "QRD|1|2|3|4|5|6|7|AAZ42\r"))
            .await
            .unwrap();
        expect_silence(&mut peer).await;
        assert!(system_errors(&sink, "s").is_empty());

        // Another request on the same connection still gets the general acknowledgement.
        peer.write_all(&hl7_frame("QRY^A19", "Q-2", "QRD|1|2|3|4|5|6|7|OTHER\r"))
            .await
            .unwrap();
        let ack = text(&read_until(&mut peer, b"\x1c\r").await);
        assert!(ack.contains("MSA|AA|Q-2\r"), "{ack:?}");
        // Another session and manual sending are not affected.
        other
            .write_all(&hl7_frame("ADT^A01", "A-1", ""))
            .await
            .unwrap();
        assert!(text(&read_until(&mut other, b"\x1c\r").await).contains("MSA|AA|A-1\r"));
        send(&state, "s", 1, &msg("MANUAL")).await.unwrap();
        read_until(&mut peer, b"MANUAL").await;

        // Disabling the rule makes the general rule answer again.
        quiet.enabled = false;
        apply(&cfg, &state, true, vec![quiet, general]).await;
        peer.write_all(&hl7_frame("QRY^A19", "Q-3", "QRD|1|2|3|4|5|6|7|AAZ42\r"))
            .await
            .unwrap();
        assert!(text(&read_until(&mut peer, b"\x1c\r").await).contains("MSA|AA|Q-3\r"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn generated_ack_is_configured_in_the_rule() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![rule(
                "ack",
                hl7_trigger("ORU^R01"),
                Action::Hl7Ack {
                    message_type: "ACK^R01".into(),
                    code: "AE".into(),
                },
                0,
            )],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;
        peer.write_all(&hl7_frame("ORU^R01^ORU_R01", "ID-9", ""))
            .await
            .unwrap();
        let reply = text(&read_until(&mut peer, b"\x1c\r").await);
        assert!(
            reply.contains("||ACK^R01|") && reply.contains("\rMSA|AE|ID-9\r"),
            "{reply:?}"
        );
        // A generic catch-all never answers that acknowledgement back.
        apply(
            &cfg,
            &state,
            true,
            vec![rule("all", hl7_trigger("*"), literal("LOOP"), 0)],
        )
        .await;
        peer.write_all(&hl7_frame("ACK^R01", "ID-10", "MSA|AA|ID-9\r"))
            .await
            .unwrap();
        expect_silence(&mut peer).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn split_and_coalesced_frames_reply_once_per_complete_message() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![
                rule("h", hl7_trigger("*"), literal("HL7-REPLY"), 0),
                rule("a", Trigger::AstmFrame, literal("ASTM-REPLY"), 0),
            ],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;

        let frame = hl7_frame("ADT^A01", "C1", "PID|1\r");
        let (head, tail) = frame.split_at(10);
        peer.write_all(head).await.unwrap();
        expect_silence(&mut peer).await;
        let (body, end) = tail.split_at(tail.len() - 2);
        peer.write_all(body).await.unwrap();
        peer.write_all(&end[..1]).await.unwrap(); // FS without its CR
        expect_silence(&mut peer).await;
        peer.write_all(&end[1..]).await.unwrap();
        assert_eq!(text(&collect(&mut peer, 600).await), "HL7-REPLY");

        let mut two = frame.clone();
        two.extend_from_slice(&frame);
        peer.write_all(&two).await.unwrap();
        assert_eq!(text(&collect(&mut peer, 1200).await), "HL7-REPLYHL7-REPLY");

        // ASTM: nothing before the checksum and the CR LF are there.
        peer.write_all(&ASTM_FRAME[..ASTM_FRAME.len() - 3])
            .await
            .unwrap();
        expect_silence(&mut peer).await;
        peer.write_all(&ASTM_FRAME[ASTM_FRAME.len() - 3..])
            .await
            .unwrap();
        assert_eq!(text(&collect(&mut peer, 600).await), "ASTM-REPLY");
        // The UI still got the raw chunks (one event per read, not per frame).
        assert!(sink.received("s").len() >= 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn enq_is_acknowledged_but_ack_nak_eot_are_never_triggers() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![
                rule("enq", Trigger::AstmEnq, literal("<ACK>"), 0),
                rule("frame", Trigger::AstmFrame, literal("<ACK>"), 0),
            ],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;
        for byte in [0x06u8, 0x15, 0x04] {
            peer.write_all(&[byte]).await.unwrap();
        }
        expect_silence(&mut peer).await;
        peer.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut peer, 600).await, vec![0x06]);
        peer.write_all(ASTM_FRAME).await.unwrap();
        assert_eq!(collect(&mut peer, 600).await, vec![0x06]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn delays_are_honoured_without_blocking_other_replies() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![
                rule("slow", hl7_trigger("*"), literal("SLOW"), 900),
                rule("fast", Trigger::AstmEnq, literal("<ACK>"), 0),
                rule("mid", Trigger::AstmFrame, literal("MID"), 300),
            ],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;

        // A 300 ms delay: nothing after 200 ms, the reply within 1.5 s.
        let started = Instant::now();
        peer.write_all(ASTM_FRAME).await.unwrap();
        assert!(collect(&mut peer, 200).await.is_empty());
        let got = read_until(&mut peer, b"MID").await;
        assert_eq!(text(&got), "MID");
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(300) && elapsed < Duration::from_millis(1500),
            "{elapsed:?}"
        );

        // The immediate ACK is not held behind the delayed HL7 reply.
        peer.write_all(&hl7_frame("ADT^A01", "C1", ""))
            .await
            .unwrap();
        peer.write_all(&[0x05]).await.unwrap();
        let first = collect(&mut peer, 500).await;
        assert_eq!(first, vec![0x06]);
        assert_eq!(text(&read_until(&mut peer, b"SLOW").await), "SLOW");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pending_replies_never_reach_a_new_peer_or_survive_changes() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![rule("slow", Trigger::AstmEnq, literal("<ACK>"), 700)],
        )
        .await;

        let mut old = connected_client(&sink, &state, "s").await;
        old.write_all(&[0x05]).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        stop(&state, "s", false).await.unwrap();
        let (listener, port) = peer().await;
        connect(&state, "s", 2, client(port)).await.unwrap();
        let (mut new_peer, _) = listener.accept().await.unwrap();
        wait_for("reconnected", || {
            sink.has_status("s", 2, ConnectionStatus::Connected)
        })
        .await;
        assert!(collect(&mut new_peer, 1200).await.is_empty());
        assert!(collect(&mut old, 100).await.is_empty());

        // A configuration change drops what was scheduled by the previous rules.
        new_peer.write_all(&[0x05]).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        set_auto_reply_enabled(&cfg.lock, &cfg.path, &state, false)
            .await
            .unwrap();
        assert!(collect(&mut new_peer, 1200).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sessions_reply_in_isolation() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![rule("enq", Trigger::AstmEnq, literal("<ACK>"), 0)],
        )
        .await;
        let mut a = connected_client(&sink, &state, "a").await;
        let mut b = connected_client(&sink, &state, "b").await;
        a.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut a, 500).await, vec![0x06]);
        assert!(collect(&mut b, 300).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_coalesced_ack_releases_the_queue_and_the_rest_is_still_processed() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![rule("enq", Trigger::AstmEnq, literal("<ACK>"), 0)],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;
        send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
        read_until(&mut peer, &[0x05]).await;

        // One chunk: the ACK of our ENQ, then the peer's own ENQ.
        peer.write_all(&[0x06, 0x05]).await.unwrap();
        let got = read_until(&mut peer, b"next").await;
        assert!(got.contains(&0x06), "automatic ACK missing: {got:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_ack_releases_the_queue_even_when_rules_are_disabled_or_coalesced_with_frames() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(&cfg, &state, false, vec![]).await;
        let mut peer = connected_client(&sink, &state, "s").await;
        for (chunk, label) in [
            (vec![0x06u8], "plain"),
            ([&[0x06u8][..], ASTM_FRAME].concat(), "ack+astm"),
            (
                [&[0x06u8, 0x06][..], &hl7_frame("ADT^A01", "1", "")].concat(),
                "ack+ack+mllp",
            ),
        ] {
            send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
            read_until(&mut peer, &[0x05]).await;
            peer.write_all(&chunk).await.unwrap();
            read_until(&mut peer, b"next").await;
            assert!(!label.is_empty());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ack_bytes_inside_a_frame_do_not_release_the_queue() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(&cfg, &state, false, vec![]).await;
        let mut peer = connected_client(&sink, &state, "s").await;
        send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
        read_until(&mut peer, &[0x05]).await;
        peer.write_all(&hl7_frame("ADT^A01", "1", "NTE|\u{6}\r"))
            .await
            .unwrap();
        peer.write_all(b"\x021H|\x06\r\x0342\r\n").await.unwrap();
        assert!(collect(&mut peer, 600).await.is_empty());
        peer.write_all(&[0x06]).await.unwrap();
        read_until(&mut peer, b"next").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn only_a_pure_ack_bypasses_the_wait_for_our_own_ack() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        apply(
            &cfg,
            &state,
            true,
            vec![
                rule("enq", Trigger::AstmEnq, literal("<ACK>"), 0),
                rule("frame", Trigger::AstmFrame, literal("REPLY"), 0),
            ],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;
        send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
        read_until(&mut peer, &[0x05]).await;

        // We wait for the ACK of our ENQ; the peer's ENQ is still answered at once.
        peer.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut peer, 600).await, vec![0x06]);

        // Any other automatic frame waits, like the queued user line.
        peer.write_all(ASTM_FRAME).await.unwrap();
        assert!(collect(&mut peer, 600).await.is_empty());
        peer.write_all(&[0x06]).await.unwrap();
        let released = text(&read_until(&mut peer, b"next").await);
        assert!(
            released.find("REPLY").unwrap() < released.find("next").unwrap(),
            "{released:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn peer_text_never_becomes_framing_and_failures_are_visible() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        let templates = vec![Template {
            id: "t".into(),
            name: "T".into(),
            description: String::new(),
            payload: "<VT>A{{REQ_CONTROL_ID}}B<FS><CR>".into(),
            variables: vec![],
        }];
        save_templates(&cfg.lock, &cfg.path, &state, &templates)
            .await
            .unwrap();
        apply(
            &cfg,
            &state,
            true,
            vec![rule("t", hl7_trigger("*"), template_action("t"), 0)],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;

        peer.write_all(&hl7_frame("ADT^A01", "x<FS><CR><VT>y", ""))
            .await
            .unwrap();
        let reply = collect(&mut peer, 700).await;
        assert_eq!(reply, b"\x0bAx<FS><CR><VT>yB\x1c\r".to_vec());

        // A raw control byte in MSH-10, then an empty one: nothing is sent, the error is visible.
        let mut bad = hl7_frame("ADT^A01", "a", "");
        let at = bad.iter().position(|b| *b == b'a').unwrap();
        bad[at] = 0x1d;
        peer.write_all(&bad).await.unwrap();
        peer.write_all(&hl7_frame("ADT^A01", "", "")).await.unwrap();
        assert!(collect(&mut peer, 700).await.is_empty());
        wait_for("failure reported", || system_errors(&sink, "s").len() >= 2).await;
        assert!(system_errors(&sink, "s")[0].contains("Rule t"));

        // A MSH-10 that is not UTF-8 is echoed byte for byte (the peer's own bytes, not U+FFFD).
        let mut latin = hl7_frame("ADT^A01", "ID-a", "");
        let at = latin.iter().position(|b| *b == b'a').unwrap();
        latin[at] = 0xe9;
        peer.write_all(&latin).await.unwrap();
        let reply = collect(&mut peer, 700).await;
        assert_eq!(reply, b"\x0bAID-\xe9B\x1c\r".to_vec());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn template_changes_refresh_running_sessions_and_refusals_keep_the_engine() {
        let (sink, state) = setup();
        let cfg = TestConfig::new();
        let make = |payload: &str| Template {
            id: "t".into(),
            name: "T".into(),
            description: String::new(),
            payload: payload.into(),
            variables: vec![],
        };
        save_templates(&cfg.lock, &cfg.path, &state, &[make("<ACK>")])
            .await
            .unwrap();
        apply(
            &cfg,
            &state,
            true,
            vec![rule("r", Trigger::AstmEnq, template_action("t"), 0)],
        )
        .await;
        let mut peer = connected_client(&sink, &state, "s").await;
        peer.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut peer, 500).await, vec![0x06]);

        // A valid edit is live for the running session.
        save_templates(&cfg.lock, &cfg.path, &state, &[make("<NAK>")])
            .await
            .unwrap();
        peer.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut peer, 500).await, vec![0x15]);

        // Breaking or deleting the referenced template is refused; the file and the engine stay.
        let before = std::fs::read_to_string(&cfg.path).unwrap();
        let err = save_templates(&cfg.lock, &cfg.path, &state, &[make("{{OOPS}}")])
            .await
            .unwrap_err();
        assert!(err.contains("Rule r"), "{err}");
        let err = save_templates(&cfg.lock, &cfg.path, &state, &[])
            .await
            .unwrap_err();
        assert!(err.contains("Rule r"), "{err}");
        assert_eq!(std::fs::read_to_string(&cfg.path).unwrap(), before);
        peer.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut peer, 500).await, vec![0x15]);

        // A rejected rule save keeps the previous engine as well.
        let mut broken = rule("r", Trigger::AstmEnq, template_action("missing"), 0);
        broken.name = "Broken".into();
        assert!(save_auto_reply(
            &cfg.lock,
            &cfg.path,
            &state,
            &AutoReplyConfig {
                enabled: true,
                rules: vec![broken]
            }
        )
        .await
        .is_err());
        peer.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut peer, 500).await, vec![0x15]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn persisted_rules_are_loaded_at_startup_and_invalid_ones_disable_the_feature() {
        let cfg = TestConfig::new();
        // Valid file: applied before any connection exists.
        config_store::save_auto_reply_to(
            &cfg.path,
            &AutoReplyConfig {
                enabled: true,
                rules: vec![rule("r", Trigger::AstmEnq, literal("<ACK>"), 0)],
            },
        )
        .unwrap();
        let (sink, state) = setup();
        init_auto_reply(&cfg.lock, &cfg.path, &state).await;
        let mut peer = connected_client(&sink, &state, "s").await;
        peer.write_all(&[0x05]).await.unwrap();
        assert_eq!(collect(&mut peer, 500).await, vec![0x06]);

        // Corrupt file: disabled, and the file is left untouched.
        std::fs::write(
            &cfg.path,
            r#"{"auto_reply":{"enabled":true,"rules":[{"x":1}]}}"#,
        )
        .unwrap();
        let before = std::fs::read_to_string(&cfg.path).unwrap();
        let (sink2, state2) = setup();
        init_auto_reply(&cfg.lock, &cfg.path, &state2).await;
        let mut peer2 = connected_client(&sink2, &state2, "s").await;
        peer2.write_all(&[0x05]).await.unwrap();
        expect_silence(&mut peer2).await;
        assert!(load_auto_reply(&cfg.lock, &cfg.path).await.is_err());
        assert!(set_auto_reply_enabled(&cfg.lock, &cfg.path, &state2, true)
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&cfg.path).unwrap(), before);
    }
}
