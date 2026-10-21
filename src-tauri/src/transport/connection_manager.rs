use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};

use super::runner::run_connection;
use crate::auto_reply::RuleSet;
use crate::emitter::Emitter;
use crate::ids::{Attempt, SessionId};
use crate::message_queue::{MessageQueue, SharedMessageQueue};
use crate::models::{ConnectionSpec, ConnectionStatus};

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
    last_attempt: Option<Attempt>,
    state: SessionState,
}

/// Registry of the independent sessions. Every method is quick and never waits for a
/// transport task, so the caller can keep it behind the global lock; waiting for a task
/// to finish is done by `StopTicket::run`, outside of that lock.
pub struct ConnectionManager {
    emitter: Emitter,
    /// The latest validated rules: every new connection starts with them.
    rules: Arc<RuleSet>,
    sessions: HashMap<SessionId, Session>,
}

/// Returned by `begin_stop`: tears one session down without holding the global lock.
pub struct StopTicket {
    session_id: SessionId,
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
            rules: Arc::new(RuleSet::disabled()),
            sessions: HashMap::new(),
        }
    }

    /// Starts connection attempt `attempt` of `session_id` (creating the session on first use).
    /// Fails, without side effects, when that session is busy or the arguments are invalid.
    pub fn connect(
        &mut self,
        session_id: &SessionId,
        attempt: Attempt,
        req: ConnectionSpec,
    ) -> Result<(), String> {
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
            if let Some(last) = session.last_attempt.filter(|last| attempt <= *last) {
                return Err(format!(
                    "Stale connection attempt {attempt} (latest is {last})"
                ));
            }
        }

        let emitter = self.emitter.scoped(session_id, attempt);
        let message_queue = MessageQueue::shared(emitter.clone(), self.rules.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connecting);
        let join = tokio::spawn(run_connection(
            emitter.clone(),
            req,
            message_queue.clone(),
            shutdown_rx,
            Arc::new(status_tx),
        ));

        let session = self.sessions.entry(session_id.clone()).or_insert(Session {
            last_attempt: None,
            state: SessionState::Idle,
        });
        // A previous, already terminated task is simply dropped: it has nothing left to release.
        session.last_attempt = Some(attempt);
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
        session_id: &SessionId,
        attempt: Attempt,
    ) -> Result<SharedMessageQueue, String> {
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
    pub fn begin_stop(
        &mut self,
        session_id: &SessionId,
        remove: bool,
    ) -> Result<StopTicket, String> {
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
            session_id: session_id.clone(),
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

    /// The rules are global: they are applied to every running session and to the future ones.
    /// Replies still pending under the previous rules are dropped.
    pub async fn apply_rules(&mut self, rules: Arc<RuleSet>) {
        self.rules = rules.clone();
        for session in self.sessions.values() {
            if let SessionState::Running(running) = &session.state {
                running.message_queue.update_rules(rules.clone()).await;
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
