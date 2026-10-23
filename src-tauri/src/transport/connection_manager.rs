use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};

use super::runner::run_connection;
use crate::auto_reply::AutoReplyRuleSet;
use crate::config_store::RevisionedConfig;
use crate::ids::{Attempt, ConnectionId, SessionId};
use crate::message_orchestrator::{MessageOrchestrator, SharedMessageOrchestrator};
use crate::models::{ConnectionSpec, ConnectionStatus};
use crate::reporting::{ConnectionReporter, ConnectionReporterFactory};

/// Longest a stop request waits for a connection task to release its socket.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

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
    attempt: Attempt,
    message_orchestrator: SharedMessageOrchestrator,
    shutdown: oneshot::Sender<()>,
    /// Always holds the latest status; terminal statuses are published before the
    /// matching event is emitted, so "terminal" here means all resources are released.
    status: watch::Receiver<ConnectionStatus>,
    join: JoinHandle<()>,
    reporter: ConnectionReporter,
}

enum SessionState {
    Idle,
    Running(Running),
    /// A stop/close is waiting for the task outside the global lock; nothing may start meanwhile.
    Stopping,
}

struct Session {
    /// Highest attempt number ever used; the frontend must always use a higher one.
    last_attempt: Option<Attempt>,
    state: SessionState,
}

/// What the manager's lock protects.
struct Registry {
    /// The latest validated rules: every new connection starts with them.
    rules: Arc<AutoReplyRuleSet>,
    /// Revision of the configuration `rules` come from; `None` until one is applied.
    config_revision: Option<u64>,
    sessions: HashMap<SessionId, Session>,
}

impl Registry {
    fn shutdown_now(&mut self) {
        for (_, session) in self.sessions.drain() {
            if let SessionState::Running(running) = session.state {
                let _ = running.shutdown.send(());
            }
        }
    }
}

/// Registry of the independent sessions, behind its own lock. The lock is only held for quick
/// operations: `stop` waits for a transport task with the lock released, so the other sessions
/// keep working meanwhile.
pub struct ConnectionManager {
    reporter_factory: ConnectionReporterFactory,
    registry: Mutex<Registry>,
}

/// Tears one session down without holding the lock.
struct StopTicket {
    session_id: SessionId,
    remove: bool,
    running: Option<Running>,
}

impl StopTicket {
    /// Signals the task and waits (bounded) until it released its socket and port.
    async fn run(&mut self) {
        let Some(mut running) = self.running.take() else {
            return;
        };
        let _ = running.shutdown.send(());
        match timeout(STOP_TIMEOUT, &mut running.join).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => running.reporter.error(
                file!(),
                line!(),
                format!("The connection task ended abnormally: {err}"),
            ),
            Err(_) => {
                running.join.abort();
                running.reporter.error(
                    file!(),
                    line!(),
                    "The connection did not stop in time and was aborted",
                );
            }
        }
    }
}

impl ConnectionManager {
    pub fn new(reporter_factory: ConnectionReporterFactory) -> Self {
        Self {
            reporter_factory,
            registry: Mutex::new(Registry {
                rules: Arc::new(AutoReplyRuleSet::disabled()),
                config_revision: None,
                sessions: HashMap::new(),
            }),
        }
    }

    /// Starts the given connection attempt (creating its session on first use).
    /// Fails, without side effects, when that session is busy or the arguments are invalid.
    pub async fn connect(
        &self,
        connection: &ConnectionId,
        req: ConnectionSpec,
    ) -> Result<(), String> {
        let session_id = &connection.session_id;
        let attempt = connection.attempt;
        let mut registry = self.registry.lock().await;
        if let Some(session) = registry.sessions.get(session_id) {
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
            if let Some(last) = session.last_attempt.filter(|last| attempt <= *last) {
                return Err(format!(
                    "Stale connection attempt {attempt} (latest is {last})"
                ));
            }
        }

        let reporter = self.reporter_factory.create(connection);
        let message_orchestrator =
            MessageOrchestrator::shared(reporter.clone(), registry.rules.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connecting);
        let join = tokio::spawn(run_connection(
            reporter.clone(),
            req,
            message_orchestrator.clone(),
            shutdown_rx,
            Arc::new(status_tx),
        ));

        let session = registry
            .sessions
            .entry(session_id.clone())
            .or_insert(Session {
                last_attempt: None,
                state: SessionState::Idle,
            });
        // A previous, already terminated task is simply dropped: it has nothing left to release.
        session.last_attempt = Some(attempt);
        session.state = SessionState::Running(Running {
            attempt,
            message_orchestrator,
            shutdown: shutdown_tx,
            status: status_rx,
            join,
            reporter,
        });
        Ok(())
    }

    /// Queue of the given attempt, only if it is really connected: nothing is ever buffered
    /// for a session that is connecting, listening or disconnected.
    pub async fn send_target(
        &self,
        connection: &ConnectionId,
    ) -> Result<SharedMessageOrchestrator, String> {
        let registry = self.registry.lock().await;
        let session = registry
            .sessions
            .get(&connection.session_id)
            .ok_or_else(|| "Unknown session".to_string())?;
        match &session.state {
            SessionState::Running(running) if running.attempt == connection.attempt => {
                let status = *running.status.borrow();
                if status == ConnectionStatus::Connected {
                    Ok(running.message_orchestrator.clone())
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

    /// Stops a session and waits until its transport released its resources.
    /// `remove`: forget the session for good instead of keeping it for a reconnect.
    pub async fn stop(&self, session_id: &SessionId, remove: bool) -> Result<(), String> {
        let mut ticket = self.begin_stop(session_id, remove).await?;
        ticket.run().await;
        self.finish_stop(&ticket).await;
        Ok(())
    }

    async fn begin_stop(&self, session_id: &SessionId, remove: bool) -> Result<StopTicket, String> {
        let mut registry = self.registry.lock().await;
        let session = registry
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
            session_id: session_id.clone(),
            remove,
            running,
        })
    }

    async fn finish_stop(&self, ticket: &StopTicket) {
        let mut registry = self.registry.lock().await;
        if ticket.remove {
            registry.sessions.remove(&ticket.session_id);
        } else if let Some(session) = registry.sessions.get_mut(&ticket.session_id) {
            session.state = SessionState::Idle;
        }
    }

    /// The rules are global: they are applied to every running session and to the future ones.
    /// Replies still pending under the previous rules are dropped. A configuration that is not
    /// newer than the one already applied is ignored (saves may reach the manager out of order);
    /// returns whether it was applied.
    pub async fn apply_rules(&self, config: &RevisionedConfig) -> bool {
        let mut registry = self.registry.lock().await;
        if registry
            .config_revision
            .is_some_and(|applied| applied >= config.revision)
        {
            return false;
        }
        registry.config_revision = Some(config.revision);
        registry.rules = config.rules.clone();
        for session in registry.sessions.values() {
            if let SessionState::Running(running) = &session.state {
                running
                    .message_orchestrator
                    .update_rules(config.rules.clone())
                    .await;
            }
        }
        true
    }

    /// Best-effort, non-blocking stop of every transport (application exit). When the lock is
    /// busy, `Drop` does it.
    pub fn shutdown_now(&self) {
        if let Ok(mut registry) = self.registry.try_lock() {
            registry.shutdown_now();
        }
    }
}

impl Drop for ConnectionManager {
    fn drop(&mut self) {
        self.registry.get_mut().shutdown_now();
    }
}
