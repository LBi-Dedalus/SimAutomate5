use std::sync::Arc;

use tokio::io::{split, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, watch};
use tokio::time::{sleep, Duration};

use crate::frames::FrameBuffer;
use crate::message_queue::SharedMessageQueue;
use crate::models::ConnectionStatus;
use crate::reporting::ConnectionReporter;
use crate::translate;

/// Any bidirectional byte stream a transport can hand over to `serve` (TCP socket, SSH channel...).
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

/// What every transport task needs, whatever the way the stream is obtained.
pub struct ConnectionContext {
    pub reporter: ConnectionReporter,
    pub message_queue: SharedMessageQueue,
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
    let message_queue = &ctx.message_queue;
    let (mut reader, mut writer) = split(stream);

    let terminal = tokio::select! {
        result = read_loop(&mut reader, message_queue.clone()) => {
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
        result = send_loop(&mut writer, message_queue.clone()) => {
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
    message_queue.close().await;
    let _ = writer.shutdown().await;
    terminal
}

async fn read_loop(
    reader: &mut (impl AsyncRead + Unpin),
    message_queue: SharedMessageQueue,
) -> Result<(), String> {
    let mut buffer = vec![0u8; 4096];
    // Complete messages are extracted here (per connection attempt) while the UI keeps
    // receiving the raw chunks.
    let mut frames = FrameBuffer::new();
    // Reassembles multibyte characters split across reads (per connection attempt).
    let mut decoder = translate::ControlTokenDecoder::new();

    loop {
        let len = reader
            .read(&mut buffer)
            .await
            .map_err(|err| err.to_string())?;
        if len == 0 {
            let pending = decoder.pending_len();
            // The dangling partial character is dropped, not emitted as a new message.
            let _ = decoder.finish();
            message_queue.log_incomplete_on_close(pending);
        }
        let res = Vec::from_iter(buffer[..len].iter().copied());
        let visible = decoder.decode(&res);
        message_queue.handle_received_message(visible, res).await?;
        for event in frames.feed(&buffer[..len]) {
            message_queue.handle_frame(event).await;
        }
    }
}

async fn send_loop(
    writer: &mut (impl AsyncWrite + Unpin),
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
