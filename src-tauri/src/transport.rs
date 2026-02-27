use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use tauri::{AppHandle, State, Emitter};
use tauri::async_runtime::{spawn, JoinHandle, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Duration};

use crate::app_state::{ActiveConnection, AppState};
use crate::auto_response;
use crate::models::{
    AutoResponseConfig, ConnectionStatus, ConnectRequest, MessageDirection, MessagePayload, Protocol,
    SendRequest, StatusPayload, MESSAGE_EVENT, MAX_RETRIES, RETRY_DELAYS, STATUS_EVENT,
};
use crate::translate;

pub async fn connect_and_spawn(app: AppHandle, state: State<'_, AppState>, req: ConnectRequest) -> Result<()> {
    let address: SocketAddr = format!("{}:{}", req.ip, req.port)
        .parse()
        .context("Invalid address")?;

    disconnect_active(&app, &state).await;

    match perform_connect(&app, address, req.retries_enabled).await {
        Ok((stream, attempts)) => {
            emit_status(&app, ConnectionStatus::Connected, attempts, None);
            let (reader, writer) = stream.into_split();
            let writer = Arc::new(Mutex::new(writer));
            let protocol = req.protocol.clone();
            let auto_response = state.auto_response.clone();
            let reader_app = app.clone();
            let reader_writer = writer.clone();
            let reader_task: JoinHandle<()> = spawn(async move {
                read_loop(
                    reader_app,
                    reader,
                    reader_writer,
                    protocol,
                    auto_response,
                )
                .await;
            });

            let mut connection = state.connection.lock().await;
            *connection = Some(ActiveConnection {
                writer,
                reader_task,
                protocol: req.protocol,
                _remote: address.to_string(),
            });

            Ok(())
        }
        Err(err) => {
            emit_status(
                &app,
                ConnectionStatus::Error,
                MAX_RETRIES,
                Some(err.to_string()),
            );
            Err(err)
        }
    }
}

pub async fn disconnect_active(app: &AppHandle, state: &State<'_, AppState>) {
    let mut connection = state.connection.lock().await;
    if let Some(active) = connection.take() {
        let mut writer = active.writer.lock().await;
        let _ = writer.shutdown().await;
        active.reader_task.abort();
    }
    emit_status(app, ConnectionStatus::Disconnected, 0, None);
}

pub async fn send_user_message(app: AppHandle, state: State<'_, AppState>, payload: SendRequest) -> Result<()> {
    let message_bytes = translate::to_bytes(&payload.message);
    let timestamp = now_ts();

    let writer = {
        let connection = state.connection.lock().await;
        if let Some(active) = connection.as_ref() {
            active.writer.clone()
        } else {
            return Err(anyhow!("No active connection"));
        }
    };

    send_bytes(&writer, &message_bytes).await?;

    emit_message(
        &app,
        MessagePayload {
            direction: MessageDirection::Sent,
            protocol: current_protocol(&state).await,
            content: translate::to_human_readable(&message_bytes),
            timestamp,
            auto_response: false,
        },
    );

    Ok(())
}

async fn perform_connect(app: &AppHandle, addr: SocketAddr, retries_enabled: bool) -> Result<(TcpStream, u32)> {
    let mut attempt: u32 = 1;

    loop {
        emit_status(app, ConnectionStatus::Connecting, attempt, None);
        let result = timeout(Duration::from_secs(1), TcpStream::connect(addr)).await;

        match result {
            Ok(Ok(stream)) => return Ok((stream, attempt)),
            Ok(Err(err)) => {
                if !retries_enabled || attempt >= MAX_RETRIES {
                    return Err(anyhow!("Connect failed: {err}"));
                }
                let backoff = RETRY_DELAYS.get(attempt as usize - 1).copied().unwrap_or(16);
                emit_status(app, ConnectionStatus::Connecting, attempt, Some(err.to_string()));
                sleep(Duration::from_secs(backoff)).await;
                attempt += 1;
            }
            Err(err) => {
                if !retries_enabled || attempt >= MAX_RETRIES {
                    return Err(anyhow!("Connect timed out: {err}"));
                }
                let backoff = RETRY_DELAYS.get(attempt as usize - 1).copied().unwrap_or(16);
                emit_status(app, ConnectionStatus::Connecting, attempt, Some(err.to_string()));
                sleep(Duration::from_secs(backoff)).await;
                attempt += 1;
            }
        }
    }
}

async fn read_loop(
    app: AppHandle,
    mut reader: tokio::net::tcp::OwnedReadHalf,
    writer: Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>,
    protocol: Protocol,
    auto_response: Arc<Mutex<AutoResponseConfig>>,
) {
    let mut buffer = vec![0u8; 4096];

    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                emit_status(&app, ConnectionStatus::Disconnected, 0, None);
                break;
            }
            Ok(len) => {
                let slice = &buffer[..len];
                let visible = translate::to_human_readable(slice);

                emit_message(
                    &app,
                    MessagePayload {
                        direction: MessageDirection::Received,
                        protocol: protocol.clone(),
                        content: visible.clone(),
                        timestamp: now_ts(),
                        auto_response: false,
                    },
                );

                let cfg = auto_response.lock().await.clone();
                if cfg.enabled {
                    if let Some(response_bytes) = auto_response::build_auto_response(&protocol, &cfg, slice) {
                        if send_bytes(&writer, &response_bytes).await.is_ok() {
                            emit_message(
                                &app,
                                MessagePayload {
                                    direction: MessageDirection::Sent,
                                    protocol: protocol.clone(),
                                    content: translate::to_human_readable(&response_bytes),
                                    timestamp: now_ts(),
                                    auto_response: true,
                                },
                            );
                        }
                    }
                }
            }
            Err(err) => {
                emit_status(&app, ConnectionStatus::Error, 0, Some(err.to_string()));
                break;
            }
        }
    }
}

async fn send_bytes(writer: &Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>, bytes: &[u8]) -> Result<()> {
    let mut guard = writer.lock().await;
    guard
        .write_all(bytes)
        .await
        .context("Failed to write to socket")?;
    Ok(())
}

async fn current_protocol(state: &State<'_, AppState>) -> Protocol {
    let connection = state.connection.lock().await;
    connection
        .as_ref()
        .map(|c| c.protocol.clone())
        .unwrap_or(Protocol::Astm)
}

fn emit_status(app: &AppHandle, status: ConnectionStatus, attempts: u32, message: Option<String>) {
    let payload = StatusPayload {
        status,
        attempts,
        message,
    };

    if let Err(err) = app.emit(STATUS_EVENT, payload) {
        eprintln!("Failed to emit status event: {err}");
    }
}

fn emit_message(app: &AppHandle, payload: MessagePayload) {
    if let Err(err) = app.emit(MESSAGE_EVENT, payload) {
        eprintln!("Failed to emit message event: {err}");
    }
}

fn now_ts() -> String {
    Utc::now().to_rfc3339()
}
