use std::sync::Arc;

use tokio::sync::{oneshot, watch};
use tokio::task::AbortHandle;

use super::ssh;
use super::streams::ConnectionContext;
use super::tcp;
use crate::emitter::Emitter;
use crate::message_queue::SharedMessageQueue;
use crate::models::{ConnectionSpec, ConnectionStatus};

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
pub(super) async fn run_connection(
    emitter: Emitter,
    spec: ConnectionSpec,
    message_queue: SharedMessageQueue,
    shutdown: oneshot::Receiver<()>,
    status: Arc<watch::Sender<ConnectionStatus>>,
) {
    emitter.emit_status(ConnectionStatus::Connecting);

    let inner = tokio::spawn(connection_task(
        emitter.clone(),
        spec,
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
    spec: ConnectionSpec,
    message_queue: SharedMessageQueue,
    shutdown: oneshot::Receiver<()>,
    status: Arc<watch::Sender<ConnectionStatus>>,
) -> ConnectionStatus {
    let mut ctx = ConnectionContext {
        emitter,
        message_queue,
        shutdown,
        status,
    };
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
