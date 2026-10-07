//! Session level operations behind the Tauri commands. They are plain functions over the shared
//! application state so that the exact same code is exercised by the tests with real sockets.
//!
//! The global state lock is only ever held for quick, non-blocking registry operations: waiting
//! for a transport task to finish happens with the lock released.

use tokio::sync::Mutex;

use crate::app_state::AppState;
use crate::models::{AutoResponseConfig, ConnectRequest, SendRequest};

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

pub async fn update_auto_response(state: &Mutex<AppState>, config: AutoResponseConfig) {
    state
        .lock()
        .await
        .connection_manager
        .update_auto_response(config)
        .await;
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
    async fn global_auto_response_applies_to_current_and_future_sessions() {
        let (sink, state) = setup();
        // Created before the configuration exists.
        let mut peer_old = connected_client(&sink, &state, "old").await;
        peer_old.write_all(&[0x05]).await.unwrap();
        expect_silence(&mut peer_old).await;

        update_auto_response(
            &state,
            AutoResponseConfig {
                enabled: true,
                astm_message: Some("<ACK>".to_string()),
                hl7_message_type: None,
                hl7_response_code: None,
            },
        )
        .await;

        // The running session picks the change up...
        peer_old.write_all(&[0x05]).await.unwrap();
        read_until(&mut peer_old, &[0x06]).await;

        // ...and a client and a server session created afterwards start with it.
        let mut peer_new = connected_client(&sink, &state, "new").await;
        peer_new.write_all(&[0x05]).await.unwrap();
        read_until(&mut peer_new, &[0x06]).await;

        let port = free_port();
        connect(&state, "srv", 1, server(port)).await.unwrap();
        wait_for("listening", || {
            sink.has_status("srv", 1, ConnectionStatus::Listening)
        })
        .await;
        let mut srv_peer = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        srv_peer.write_all(&[0x05]).await.unwrap();
        read_until(&mut srv_peer, &[0x06]).await;
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
}
