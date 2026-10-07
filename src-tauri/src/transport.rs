use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::{sleep, timeout, Duration};

use crate::emitter::Emitter;
use crate::message_queue::{MessageQueue, SharedMessageQueue};
use crate::models::{AutoResponseConfig, ConnectRequest, ConnectionStatus, LogLevel};

/// Longest a stop request waits for a connection task to release its socket.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SESSION_ID_LEN: usize = 64;

/// Checks that a frontend supplied session id is usable (and safe to log).
pub fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty() || session_id.len() > MAX_SESSION_ID_LEN {
        return Err(format!(
            "Invalid session id: it must contain 1 to {MAX_SESSION_ID_LEN} characters"
        ));
    }
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Invalid session id: only letters, digits, '-' and '_' are allowed".into());
    }
    Ok(())
}

fn is_terminal(status: ConnectionStatus) -> bool {
    matches!(
        status,
        ConnectionStatus::Disconnected | ConnectionStatus::Error
    )
}

fn status_label(status: ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Disconnected => "disconnected",
        ConnectionStatus::Connecting => "connecting",
        ConnectionStatus::Listening => "listening",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::Error => "in error",
    }
}

/// A connection attempt that is (or was until recently) executing.
struct Running {
    attempt: u64,
    message_queue: SharedMessageQueue,
    shutdown: oneshot::Sender<()>,
    /// Always holds the latest status; terminal statuses are published before the
    /// matching event is emitted, so "terminal" here means all resources are released.
    status: watch::Receiver<ConnectionStatus>,
    join: JoinHandle<()>,
    emitter: Emitter,
}

enum SessionState {
    Idle,
    Running(Running),
    /// A stop/close is waiting for the task outside the global lock; nothing may start meanwhile.
    Stopping,
}

struct Session {
    /// Highest attempt number ever used; the frontend must always use a higher one.
    last_attempt: u64,
    state: SessionState,
}

/// Registry of the independent sessions. Every method is quick and never waits for a
/// transport task, so the caller can keep it behind the global lock; waiting for a task
/// to finish is done by `StopTicket::run`, outside of that lock.
pub struct ConnectionManager {
    emitter: Emitter,
    auto_response: AutoResponseConfig,
    sessions: HashMap<String, Session>,
}

/// Returned by `begin_stop`: tears one session down without holding the global lock.
pub struct StopTicket {
    session_id: String,
    remove: bool,
    running: Option<Running>,
}

impl StopTicket {
    /// Signals the task and waits (bounded) until it released its socket and port.
    pub async fn run(&mut self) {
        let Some(mut running) = self.running.take() else {
            return;
        };
        let _ = running.shutdown.send(());
        match timeout(STOP_TIMEOUT, &mut running.join).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => running.emitter.error(
                file!(),
                line!(),
                format!("The connection task ended abnormally: {err}"),
            ),
            Err(_) => {
                running.join.abort();
                running.emitter.error(
                    file!(),
                    line!(),
                    "The connection did not stop in time and was aborted",
                );
            }
        }
    }
}

impl ConnectionManager {
    pub fn new(emitter: Emitter) -> Self {
        Self {
            emitter,
            auto_response: AutoResponseConfig::default(),
            sessions: HashMap::new(),
        }
    }

    /// Starts connection attempt `attempt` of `session_id` (creating the session on first use).
    /// Fails, without side effects, when that session is busy or the arguments are invalid.
    pub fn connect(
        &mut self,
        session_id: &str,
        attempt: u64,
        req: ConnectRequest,
    ) -> Result<(), String> {
        validate_session_id(session_id)?;
        validate_request(&req)?;
        if attempt == 0 {
            return Err("Invalid attempt: it must be greater than zero".into());
        }

        if let Some(session) = self.sessions.get(session_id) {
            match &session.state {
                SessionState::Stopping => {
                    return Err("This session is being stopped, try again in a moment".into())
                }
                SessionState::Running(running) => {
                    let status = *running.status.borrow();
                    if !is_terminal(status) {
                        return Err(format!(
                            "This session is already {}; disconnect it first",
                            status_label(status)
                        ));
                    }
                }
                SessionState::Idle => {}
            }
            if attempt <= session.last_attempt {
                return Err(format!(
                    "Stale connection attempt {attempt} (latest is {})",
                    session.last_attempt
                ));
            }
        }

        let emitter = self.emitter.scoped(session_id, attempt);
        let message_queue = MessageQueue::shared(emitter.clone(), self.auto_response.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connecting);
        let join = tokio::spawn(run_connection(
            emitter.clone(),
            req,
            message_queue.clone(),
            shutdown_rx,
            Arc::new(status_tx),
        ));

        let session = self
            .sessions
            .entry(session_id.to_string())
            .or_insert(Session {
                last_attempt: 0,
                state: SessionState::Idle,
            });
        // A previous, already terminated task is simply dropped: it has nothing left to release.
        session.last_attempt = attempt;
        session.state = SessionState::Running(Running {
            attempt,
            message_queue,
            shutdown: shutdown_tx,
            status: status_rx,
            join,
            emitter,
        });
        Ok(())
    }

    /// Queue of the given attempt, only if it is really connected: nothing is ever buffered
    /// for a session that is connecting, listening or disconnected.
    pub fn send_target(
        &self,
        session_id: &str,
        attempt: u64,
    ) -> Result<SharedMessageQueue, String> {
        validate_session_id(session_id)?;
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| "Unknown session".to_string())?;
        match &session.state {
            SessionState::Running(running) if running.attempt == attempt => {
                let status = *running.status.borrow();
                if status == ConnectionStatus::Connected {
                    Ok(running.message_queue.clone())
                } else {
                    Err(format!(
                        "Cannot send: the session is {}, not connected",
                        status_label(status)
                    ))
                }
            }
            SessionState::Running(_) => {
                Err("Cannot send: this connection attempt is not the current one".into())
            }
            SessionState::Idle => Err("Cannot send: the session is disconnected".into()),
            SessionState::Stopping => Err("Cannot send: the session is being stopped".into()),
        }
    }

    /// Starts stopping a session (`remove`: close it for good). The caller must then drop the
    /// global lock, `run` the ticket and call `finish_stop`.
    pub fn begin_stop(&mut self, session_id: &str, remove: bool) -> Result<StopTicket, String> {
        validate_session_id(session_id)?;
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| "Unknown session".to_string())?;
        if matches!(session.state, SessionState::Stopping) {
            return Err("This session is already being stopped".into());
        }
        let running = match std::mem::replace(&mut session.state, SessionState::Stopping) {
            SessionState::Running(running) => Some(running),
            _ => None,
        };
        Ok(StopTicket {
            session_id: session_id.to_string(),
            remove,
            running,
        })
    }

    pub fn finish_stop(&mut self, ticket: &StopTicket) {
        if ticket.remove {
            self.sessions.remove(&ticket.session_id);
        } else if let Some(session) = self.sessions.get_mut(&ticket.session_id) {
            session.state = SessionState::Idle;
        }
    }

    /// The configuration is global: it is applied to every running session and to the future ones.
    pub async fn update_auto_response(&mut self, config: AutoResponseConfig) {
        self.auto_response = config.clone();
        for session in self.sessions.values() {
            if let SessionState::Running(running) = &session.state {
                running
                    .message_queue
                    .update_auto_response(config.clone())
                    .await;
            }
        }
    }

    /// Best-effort, non-blocking stop of every transport (application exit).
    pub fn shutdown_now(&mut self) {
        for (_, session) in self.sessions.drain() {
            if let SessionState::Running(running) = session.state {
                let _ = running.shutdown.send(());
            }
        }
    }
}

fn validate_request(req: &ConnectRequest) -> Result<(), String> {
    match req {
        ConnectRequest::ClientConnectRequest { ip, port } => {
            if ip.trim().is_empty() {
                return Err("The host is required".into());
            }
            if *port == 0 {
                return Err("The port must be between 1 and 65535".into());
            }
        }
        ConnectRequest::ServerStartRequest { port } => {
            if *port == 0 {
                return Err("The port must be between 1 and 65535".into());
            }
        }
    }
    Ok(())
}

/// Aborts the wrapped task when dropped (the connection task is isolated in its own task).
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs one attempt to completion. It never touches the application state: its only outputs are
/// scoped events and the watch channel, and it always ends by publishing a terminal status,
/// whatever happens (including a panic of the transport code).
async fn run_connection(
    emitter: Emitter,
    req: ConnectRequest,
    message_queue: SharedMessageQueue,
    shutdown: oneshot::Receiver<()>,
    status: Arc<watch::Sender<ConnectionStatus>>,
) {
    emitter.emit_status(ConnectionStatus::Connecting);

    let inner = tokio::spawn(connection_task(
        emitter.clone(),
        req,
        message_queue,
        shutdown,
        status.clone(),
    ));
    let _guard = AbortOnDrop(inner.abort_handle());
    let terminal = match inner.await {
        Ok(terminal) => terminal,
        Err(err) => {
            emitter.error(
                file!(),
                line!(),
                format!("The connection task failed unexpectedly: {err}"),
            );
            ConnectionStatus::Error
        }
    };
    // Published first: once the frontend sees the terminal event, a reconnect is accepted.
    status.send_replace(terminal);
    emitter.emit_status(terminal);
}

async fn connection_task(
    emitter: Emitter,
    req: ConnectRequest,
    message_queue: SharedMessageQueue,
    mut shutdown: oneshot::Receiver<()>,
    status: Arc<watch::Sender<ConnectionStatus>>,
) -> ConnectionStatus {
    match req {
        ConnectRequest::ClientConnectRequest { ip, port } => {
            client_task(
                &emitter,
                format!("{}:{}", ip.trim(), port),
                message_queue,
                &mut shutdown,
                &status,
            )
            .await
        }
        ConnectRequest::ServerStartRequest { port } => {
            server_task(&emitter, port, message_queue, &mut shutdown, &status).await
        }
    }
}

async fn client_task(
    emitter: &Emitter,
    addr: String,
    message_queue: SharedMessageQueue,
    shutdown: &mut oneshot::Receiver<()>,
    status: &watch::Sender<ConnectionStatus>,
) -> ConnectionStatus {
    emitter.info(file!(), line!(), format!("Connecting to {}...", &addr));

    let stream = tokio::select! {
        res = loop_till_connect(emitter, addr.clone()) => {
            match res {
                Ok(tcp_stream) => tcp_stream,
                Err(_) => return ConnectionStatus::Error,
            }
        }
        _ = &mut *shutdown => {
            emitter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    emitter.info(file!(), line!(), format!("Connected to {}...", &addr));
    status.send_replace(ConnectionStatus::Connected);
    emitter.emit_status(ConnectionStatus::Connected);
    emitter.emit_notification("Connected", &format!("Connected to {}", &addr));

    run_connected(emitter, stream, message_queue, shutdown).await
}

async fn server_task(
    emitter: &Emitter,
    port: u16,
    message_queue: SharedMessageQueue,
    shutdown: &mut oneshot::Receiver<()>,
    status: &watch::Sender<ConnectionStatus>,
) -> ConnectionStatus {
    emitter.info(
        file!(),
        line!(),
        format!("Starting server on port {}...", port),
    );
    let addr = format!("0.0.0.0:{}", port);

    let listener = tokio::select! {
        res = TcpListener::bind(addr.clone()) => {
            match res {
                Ok(listener) => listener,
                Err(err) => {
                    emitter.error(
                        file!(),
                        line!(),
                        format!("Error while starting server {}", err),
                    );
                    return ConnectionStatus::Error;
                }
            }
        }
        _ = &mut *shutdown => {
            emitter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    // The port is really bound: only now is the server ready.
    emitter.info(
        file!(),
        line!(),
        format!("Server listening on {}, waiting for a client...", &addr),
    );
    status.send_replace(ConnectionStatus::Listening);
    emitter.emit_status(ConnectionStatus::Listening);

    let stream = tokio::select! {
        res = listener.accept() => {
            match res {
                Ok((tcp_stream, client_addr)) => {
                    emitter.info(
                        file!(),
                        line!(),
                        format!("Client connected address={}", &client_addr),
                    );
                    tcp_stream
                }
                Err(err) => {
                    emitter.error(
                        file!(),
                        line!(),
                        format!("Error while accepting connection {}", err),
                    );
                    return ConnectionStatus::Error;
                }
            }
        }
        _ = &mut *shutdown => {
            emitter.info(
                file!(),
                line!(),
                "Server stopped while waiting for a client",
            );
            return ConnectionStatus::Disconnected;
        }
    };
    // Only one client is served: free the port as soon as it is accepted.
    drop(listener);

    let peer_addr = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or("unknown".to_string());
    emitter.info(
        file!(),
        line!(),
        format!(
            "Client from {} connected, listening on {}...",
            peer_addr, &addr
        ),
    );
    status.send_replace(ConnectionStatus::Connected);
    emitter.emit_status(ConnectionStatus::Connected);
    emitter.emit_notification(
        "Connected",
        &format!("Accepted a connection from {} on {}", peer_addr, &addr),
    );

    run_connected(emitter, stream, message_queue, shutdown).await
}

/// Exchanges data until EOF, an I/O error or a shutdown request.
async fn run_connected(
    emitter: &Emitter,
    stream: TcpStream,
    message_queue: SharedMessageQueue,
    shutdown: &mut oneshot::Receiver<()>,
) -> ConnectionStatus {
    let (mut reader, mut writer) = stream.into_split();

    let terminal = tokio::select! {
        result = read_loop(&mut reader, message_queue.clone()) => {
            match result {
                Err(err) => {
                    emitter.error(
                        file!(),
                        line!(),
                        format!("An error occurred while reading, disconnecting: {err}"),
                    );
                    ConnectionStatus::Error
                }
                Ok(()) => ConnectionStatus::Disconnected,
            }
        }
        result = send_loop(&mut writer, message_queue) => {
            match result {
                Err(err) => {
                    emitter.error(
                        file!(),
                        line!(),
                        format!("An error occurred while writing, disconnecting: {err}"),
                    );
                    ConnectionStatus::Error
                }
                Ok(()) => ConnectionStatus::Disconnected,
            }
        }
        _ = &mut *shutdown => {
            emitter.info(file!(), line!(), "Disconnected successfully");
            ConnectionStatus::Disconnected
        }
    };

    let _ = writer.shutdown().await;
    terminal
}

async fn loop_till_connect(emitter: &Emitter, addr: String) -> Result<TcpStream, ()> {
    let mut attempt = 1;
    loop {
        emitter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("connect attempt={} address={}", attempt, &addr),
        );

        let result = timeout(Duration::from_secs(1), TcpStream::connect(addr.clone())).await;

        match result {
            Ok(Ok(tcp_stream)) => {
                emitter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("connect succeeded attempt={} address={}", attempt, &addr),
                );
                return Ok(tcp_stream);
            }
            Ok(Err(err)) => {
                emitter.error(
                    file!(),
                    line!(),
                    format!("Connection to {} failed: {}", &addr, err.to_string()),
                );
                return Err(());
            }
            Err(_err) => {
                emitter.warn(
                    file!(),
                    line!(),
                    format!("Connection to {} timed out: attempt={}", &addr, attempt,),
                );
            }
        };

        sleep(Duration::from_secs(1)).await;
        attempt += 1;
    }
}

async fn read_loop(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    message_queue: SharedMessageQueue,
) -> Result<(), String> {
    let mut buffer = vec![0u8; 4096];

    loop {
        let len = reader
            .read(&mut buffer)
            .await
            .map_err(|err| err.to_string())?;
        let res = Vec::from_iter(buffer[..len].iter().copied());
        message_queue.handle_received_message(res).await?;
    }
}

async fn send_loop(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    message_queue: SharedMessageQueue,
) -> Result<(), String> {
    loop {
        let msg = message_queue.recv().await;
        let mut pos = 0;

        while pos < msg.len() {
            let len = writer
                .write(&msg[pos..])
                .await
                .map_err(|err| err.to_string())?;
            if len == 0 {
                return Err("Connection closed by peer".to_string());
            }
            pos += len;
        }

        message_queue.handle_sent_message(&msg);
        sleep(Duration::from_millis(200)).await;
    }
}
