use std::collections::HashMap;
use std::sync::{Arc, Weak};

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

const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const FORCED_CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_INCOMPLETE: &str = "Stop incomplete: cleanup could not be confirmed within the \
15-second forced-cleanup period. Sending and reconnecting remain unavailable until cleanup completes.";

type StopOutcome = Option<Result<(), String>>;

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
        ConnectionStatus::Stopping => "stopping",
        ConnectionStatus::Error => "in error",
    }
}

/// A connection attempt that is (or was until recently) executing.
struct Running {
    attempt: Attempt,
    message_orchestrator: SharedMessageOrchestrator,
    shutdown: Option<oneshot::Sender<()>>,
    force_stop: watch::Sender<bool>,
    /// Always holds the latest status; terminal statuses are published before the
    /// matching event is emitted, so "terminal" here means all resources are released.
    status: Arc<watch::Sender<ConnectionStatus>>,
    _join: JoinHandle<()>,
    reporter: ConnectionReporter,
}

enum SessionState {
    Idle,
    Running(Running),
    /// A stop/close is waiting for the task outside the global lock; nothing may start meanwhile.
    Stopping(Stopping),
}

struct Stopping {
    running: Running,
    remove: bool,
    /// Once an incomplete result is committed it is never replaced by success.
    outcome: watch::Sender<StopOutcome>,
    _monitor: JoinHandle<()>,
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
            let running = match session.state {
                SessionState::Running(running) => running,
                SessionState::Stopping(stopping) => stopping.running,
                SessionState::Idle => continue,
            };
            if let Some(shutdown) = running.shutdown {
                let _ = shutdown.send(());
            }
            running.force_stop.send_replace(true);
        }
    }
}

/// Registry of the independent sessions, behind its own lock. The lock is only held for quick
/// operations: `stop` waits for a transport task with the lock released, so the other sessions
/// keep working meanwhile.
pub struct ConnectionManager {
    reporter_factory: ConnectionReporterFactory,
    registry: Arc<Mutex<Registry>>,
}

impl ConnectionManager {
    pub fn new(reporter_factory: ConnectionReporterFactory) -> Self {
        Self {
            reporter_factory,
            registry: Arc::new(Mutex::new(Registry {
                rules: Arc::new(AutoReplyRuleSet::disabled()),
                config_revision: None,
                sessions: HashMap::new(),
            })),
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
                SessionState::Stopping(_) => {
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
        let (force_tx, force_rx) = watch::channel(false);
        let (status_tx, _) = watch::channel(ConnectionStatus::Connecting);
        let status_tx = Arc::new(status_tx);
        let join = spawn_attempt(
            Arc::downgrade(&self.registry),
            connection.clone(),
            reporter.clone(),
            status_tx.clone(),
            run_connection(
                reporter.clone(),
                req,
                message_orchestrator.clone(),
                shutdown_rx,
                status_tx.clone(),
                force_rx,
            ),
        );

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
            shutdown: Some(shutdown_tx),
            force_stop: force_tx,
            status: status_tx,
            _join: join,
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
            SessionState::Stopping(_) => Err("Cannot send: the session is being stopped".into()),
        }
    }

    /// Stops a session and waits until its transport released its resources.
    /// `remove`: forget the session for good instead of keeping it for a reconnect.
    pub async fn stop(&self, session_id: &SessionId, remove: bool) -> Result<(), String> {
        self.stop_with_timeouts(session_id, remove, STOP_TIMEOUT, FORCED_CLEANUP_TIMEOUT)
            .await
    }

    async fn stop_with_timeouts(
        &self,
        session_id: &SessionId,
        remove: bool,
        graceful: Duration,
        forced: Duration,
    ) -> Result<(), String> {
        let Some(mut outcome) = self
            .begin_stop(session_id, remove, graceful, forced)
            .await?
        else {
            return Ok(());
        };
        let result = outcome
            .wait_for(|result| result.is_some())
            .await
            .map_err(|_| "The stop monitor ended without confirming cleanup".to_string())?;
        result.as_ref().expect("waited for a stop outcome").clone()
    }

    async fn begin_stop(
        &self,
        session_id: &SessionId,
        remove: bool,
        graceful: Duration,
        forced: Duration,
    ) -> Result<Option<watch::Receiver<StopOutcome>>, String> {
        let mut registry = self.registry.lock().await;
        let session = registry
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| "Unknown session".to_string())?;
        if matches!(session.state, SessionState::Stopping(_)) {
            return Err("This session is already being stopped".into());
        }
        let mut running = match std::mem::replace(&mut session.state, SessionState::Idle) {
            SessionState::Running(running) => running,
            SessionState::Idle => {
                if remove {
                    registry.sessions.remove(session_id);
                }
                return Ok(None);
            }
            SessionState::Stopping(_) => unreachable!("checked above"),
        };
        // The worker may still be publishing setup progress, but cannot overwrite this status.
        // The same registry lock orders this event with completion and reconnect admission.
        running.status.send_replace(ConnectionStatus::Stopping);
        let status = running.status.subscribe();
        running.reporter.emit_status(ConnectionStatus::Stopping);
        let (outcome, result) = watch::channel(None);
        let monitor = tokio::spawn(monitor_stop(
            Arc::downgrade(&self.registry),
            session_id.clone(),
            running.attempt,
            status,
            running.force_stop.clone(),
            graceful,
            forced,
        ));
        if let Some(shutdown) = running.shutdown.take() {
            let _ = shutdown.send(());
        }
        session.state = SessionState::Stopping(Stopping {
            remove,
            running,
            outcome,
            _monitor: monitor,
        });
        Ok(Some(result))
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

    /// Best-effort, non-blocking stop of every transport (application exit).
    pub fn shutdown_now(&self) {
        if let Ok(mut registry) = self.registry.try_lock() {
            registry.shutdown_now();
        }
    }
}

impl Drop for ConnectionManager {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

fn spawn_attempt(
    registry: Weak<Mutex<Registry>>,
    connection: ConnectionId,
    reporter: ConnectionReporter,
    status: Arc<watch::Sender<ConnectionStatus>>,
    attempt: impl std::future::Future<Output = ConnectionStatus> + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let terminal = attempt.await;
        let Some(registry) = registry.upgrade() else {
            return;
        };
        let mut registry = registry.lock().await;
        let Some(session) = registry.sessions.get_mut(&connection.session_id) else {
            return;
        };
        if session.last_attempt != Some(connection.attempt) {
            return;
        }
        let mut remove = false;
        if let SessionState::Stopping(stopping) =
            std::mem::replace(&mut session.state, SessionState::Idle)
        {
            let incomplete = stopping
                .outcome
                .borrow()
                .as_ref()
                .is_some_and(Result::is_err);
            remove = stopping.remove && !incomplete;
            if incomplete {
                reporter.info(
                    file!(),
                    line!(),
                    "Cleanup completed. The connection is now stopped.",
                );
            } else {
                if *stopping.running.force_stop.borrow() {
                    reporter.warn(
                        file!(),
                        line!(),
                        "Forced stopping was needed; local cleanup is complete.",
                    );
                } else {
                    reporter.info(file!(), line!(), "Disconnected successfully");
                }
                stopping.outcome.send_replace(Some(Ok(())));
            }
        }
        if remove {
            registry.sessions.remove(&connection.session_id);
        }
        // Completion and command outcomes are committed under the same lock as the timeout
        // decision: a timed-out Close can never later remove the retained session.
        status.send_replace(terminal);
        reporter.emit_status(terminal);
    })
}

async fn monitor_stop(
    registry: Weak<Mutex<Registry>>,
    session_id: SessionId,
    attempt: Attempt,
    mut status: watch::Receiver<ConnectionStatus>,
    force_stop: watch::Sender<bool>,
    graceful: Duration,
    forced: Duration,
) {
    if matches!(
        timeout(graceful, status.wait_for(|status| is_terminal(*status))).await,
        Ok(Ok(_))
    ) {
        return;
    }

    force_stop.send_replace(true);
    if matches!(
        timeout(forced, status.wait_for(|status| is_terminal(*status))).await,
        Ok(Ok(_))
    ) {
        return;
    }
    let Some(registry) = registry.upgrade() else {
        return;
    };
    let mut registry = registry.lock().await;
    if let Some(Session {
        state: SessionState::Stopping(stopping),
        ..
    }) = registry.sessions.get_mut(&session_id)
    {
        if stopping.running.attempt == attempt && stopping.outcome.borrow().is_none() {
            stopping.remove = false;
            stopping
                .running
                .reporter
                .error(file!(), line!(), STOP_INCOMPLETE);
            stopping
                .outcome
                .send_replace(Some(Err(STOP_INCOMPLETE.to_string())));
        }
    }
}

#[cfg(test)]
mod tests;
