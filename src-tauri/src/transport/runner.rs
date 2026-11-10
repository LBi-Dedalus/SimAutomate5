use std::sync::Arc;

use tokio::sync::{oneshot, watch};
use tokio::task::AbortHandle;

use super::cleanup::{wait_for_force, AttemptCleanup};
use super::ssh;
use super::streams::ConnectionContext;
use super::tcp;
use crate::message_orchestrator::SharedMessageOrchestrator;
use crate::models::{ConnectionSpec, ConnectionStatus};
use crate::reporting::ConnectionReporter;

/// Aborts the wrapped task when dropped (the connection task is isolated in its own task).
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The supervisor outlives forced worker cancellation and observes all attempt-owned cleanup.
/// The manager publishes the returned terminal status only after finalizing its registry.
pub(super) async fn run_connection(
    reporter: ConnectionReporter,
    spec: ConnectionSpec,
    message_orchestrator: SharedMessageOrchestrator,
    shutdown: oneshot::Receiver<()>,
    status: Arc<watch::Sender<ConnectionStatus>>,
    force_stop: watch::Receiver<bool>,
) -> ConnectionStatus {
    let cleanup = AttemptCleanup::new(force_stop.clone());
    let worker = tokio::spawn(connection_task(
        reporter.clone(),
        spec,
        message_orchestrator.clone(),
        shutdown,
        status.clone(),
        cleanup.clone(),
    ));
    supervise(
        worker,
        message_orchestrator,
        cleanup,
        force_stop,
        status,
        &reporter,
    )
    .await
}

pub(super) async fn supervise(
    mut worker: tokio::task::JoinHandle<ConnectionStatus>,
    message_orchestrator: SharedMessageOrchestrator,
    cleanup: AttemptCleanup,
    mut force_stop: watch::Receiver<bool>,
    status: Arc<watch::Sender<ConnectionStatus>>,
    reporter: &ConnectionReporter,
) -> ConnectionStatus {
    let _guard = AbortOnDrop(worker.abort_handle());
    let mut forced = false;
    let result = tokio::select! {
        biased;
        result = &mut worker => result,
        _ = wait_for_force(&mut force_stop) => {
            forced = true;
            worker.abort();
            worker.await
        }
    };
    let terminal = match result {
        Ok(terminal) => terminal,
        Err(err) if forced && err.is_cancelled() => ConnectionStatus::Disconnected,
        Err(err) => {
            reporter.error(
                file!(),
                line!(),
                format!("The connection task failed unexpectedly: {err}"),
            );
            ConnectionStatus::Error
        }
    };
    if status.send_if_modified(|current| {
        if *current == ConnectionStatus::Stopping {
            return false;
        }
        *current = ConnectionStatus::Stopping;
        true
    }) {
        reporter.emit_status(ConnectionStatus::Stopping);
    }
    message_orchestrator.close().await;
    if cleanup.finish(reporter).await {
        terminal
    } else {
        ConnectionStatus::Error
    }
}

async fn connection_task(
    reporter: ConnectionReporter,
    spec: ConnectionSpec,
    message_orchestrator: SharedMessageOrchestrator,
    shutdown: oneshot::Receiver<()>,
    status: Arc<watch::Sender<ConnectionStatus>>,
    cleanup: AttemptCleanup,
) -> ConnectionStatus {
    let mut ctx = ConnectionContext {
        reporter,
        message_orchestrator,
        shutdown,
        status,
        cleanup,
    };
    ctx.publish_status(ConnectionStatus::Connecting);
    match spec {
        ConnectionSpec::Client { host, port } => {
            tcp::client_task(&mut ctx, format!("{}:{}", host.as_str(), port.get())).await
        }
        ConnectionSpec::Server { port } => tcp::server_task(&mut ctx, port.get()).await,
        ConnectionSpec::SshClient { host, port, tunnel } => {
            ssh::client_task(&mut ctx, &tunnel, &host, port).await
        }
        ConnectionSpec::SshServer { port, tunnel } => {
            ssh::server_task(&mut ctx, &tunnel, port).await
        }
    }
}
