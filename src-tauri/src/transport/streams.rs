use std::sync::Arc;

use tokio::io::{split, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, watch};
use tokio::time::{sleep, Duration};

use crate::message_orchestrator::SharedMessageOrchestrator;
use crate::models::ConnectionStatus;
use crate::reporting::ConnectionReporter;

/// Any bidirectional byte stream a transport can hand over to `serve` (TCP socket, SSH channel...).
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

/// What every transport task needs, whatever the way the stream is obtained.
pub struct ConnectionContext {
    pub reporter: ConnectionReporter,
    pub message_orchestrator: SharedMessageOrchestrator,
    pub shutdown: oneshot::Receiver<()>,
    pub status: Arc<watch::Sender<ConnectionStatus>>,
}

/// Publishes the Connected status, then exchanges data over `stream` until the end of the connection.
pub async fn serve(
    ctx: &mut ConnectionContext,
    stream: impl AsyncStream,
    notification: &str,
) -> ConnectionStatus {
    ctx.status.send_replace(ConnectionStatus::Connected);
    ctx.reporter.emit_status(ConnectionStatus::Connected);
    ctx.reporter.emit_notification("Connected", notification);

    run_connected(ctx, stream).await
}

/// Exchanges data until EOF, an I/O error or a shutdown request.
async fn run_connected(ctx: &mut ConnectionContext, stream: impl AsyncStream) -> ConnectionStatus {
    let reporter = &ctx.reporter;
    let message_orchestrator = &ctx.message_orchestrator;
    let (mut reader, mut writer) = split(stream);

    let terminal = tokio::select! {
        result = read_loop(&mut reader, message_orchestrator.clone()) => {
            match result {
                Err(err) => {
                    reporter.error(
                        file!(),
                        line!(),
                        format!("An error occurred while reading, disconnecting: {err}"),
                    );
                    ConnectionStatus::Error
                }
                Ok(()) => ConnectionStatus::Disconnected,
            }
        }
        result = send_loop(&mut writer, message_orchestrator.clone()) => {
            match result {
                Err(err) => {
                    reporter.error(
                        file!(),
                        line!(),
                        format!("An error occurred while writing, disconnecting: {err}"),
                    );
                    ConnectionStatus::Error
                }
                Ok(()) => ConnectionStatus::Disconnected,
            }
        }
        _ = &mut ctx.shutdown => {
            reporter.info(file!(), line!(), "Disconnected successfully");
            ConnectionStatus::Disconnected
        }
    };

    // Nothing pending (user messages, delayed automatic replies) may outlive the connection.
    message_orchestrator.close().await;
    let _ = writer.shutdown().await;
    terminal
}

async fn read_loop(
    reader: &mut (impl AsyncRead + Unpin),
    message_orchestrator: SharedMessageOrchestrator,
) -> Result<(), String> {
    let mut buffer = vec![0u8; 4096];

    loop {
        let len = reader
            .read(&mut buffer)
            .await
            .map_err(|err| err.to_string())?;
        message_orchestrator.on_received(&buffer[..len]).await?;
    }
}

async fn send_loop(
    writer: &mut (impl AsyncWrite + Unpin),
    message_orchestrator: SharedMessageOrchestrator,
) -> Result<(), String> {
    loop {
        let msg = message_orchestrator.recv().await;
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

        message_orchestrator.handle_sent_message(&msg);
        sleep(Duration::from_millis(200)).await;
    }
}
