use chrono::Utc;
use std::io;
use std::net::SocketAddr;
use tauri::async_runtime::spawn;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Duration};

use crate::auto_response;
use crate::logger::AppLogger;
use crate::models::{
    AutoResponseConfig, ConnectRequest, ConnectionStatus, LogLevel, MessageDirection,
    MessagePayload, SendRequest, StatusPayload, MAX_RETRIES, MESSAGE_EVENT, RETRY_DELAYS,
    STATUS_EVENT,
};
use crate::translate;

pub type ConnectionQueue = tokio::sync::mpsc::Sender<ConnectionMessage>;
pub type ConnectionQueueReceiver = tokio::sync::mpsc::Receiver<ConnectionMessage>;

pub enum ConnectionMessage {
    Disconnect,
    SendMessage(SendRequest),
    SetAutoResponse(AutoResponseConfig),
    MessageReceived(Result<Vec<u8>, io::Error>),
}

pub async fn start(app: &AppHandle, req: ConnectRequest, logger: AppLogger) -> Option<ConnectionQueue> {
    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        format!("starting transport for {}:{}", req.ip, req.port),
    );

    let address: SocketAddr = match format!("{}:{}", req.ip, req.port).parse() {
        Ok(addr) => addr,
        Err(err) => {
            emit_status(
                &app,
                &logger,
                ConnectionStatus::Error,
                0,
                Some(format!("Invalid address: {err}")),
            );
            logger.log_backend(
                LogLevel::Err,
                file!(),
                line!(),
                format!("invalid socket address for transport start: {err}"),
            );
            return None;
        }
    };

    let stream = perform_connect(&app, &logger, address, req.retries_enabled).await?;
    let (reader, writer) = stream.into_split();
    let (tx, rx) = tokio::sync::mpsc::channel::<ConnectionMessage>(100);

    spawn(read_loop(reader, tx.clone()));
    spawn(receive_loop(writer, rx, app.clone(), logger));

    Some(tx.clone())
}

async fn perform_connect(
    app: &AppHandle,
    logger: &AppLogger,
    addr: SocketAddr,
    retries_enabled: bool,
) -> Option<TcpStream> {
    let mut attempt: u32 = 1;
    emit_status(app, logger, ConnectionStatus::Connecting, attempt, None);

    loop {
        logger.log_backend(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("connect attempt={} address={} retries_enabled={}", attempt, addr, retries_enabled),
        );

        let result = timeout(Duration::from_secs(1), TcpStream::connect(addr)).await;

        let err = match result {
            Ok(Ok(stream)) => {
                emit_status(&app, logger, ConnectionStatus::Connected, attempt, None);
                logger.log_backend(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("connect succeeded attempt={} address={}", attempt, addr),
                );
                return Some(stream);
            }
            Ok(Err(err)) => err.to_string(),
            Err(err) => err.to_string(),
        };

        logger.log_backend(
            LogLevel::Wrn,
            file!(),
            line!(),
            format!("connect failed attempt={} address={} error={}", attempt, addr, err),
        );

        if !retries_enabled || attempt >= MAX_RETRIES {
            emit_status(
                &app,
                logger,
                ConnectionStatus::Error,
                MAX_RETRIES,
                Some(err.to_string()),
            );
            logger.log_backend(
                LogLevel::Err,
                file!(),
                line!(),
                format!("connect giving up after attempts={} address={}", attempt, addr),
            );
            return None;
        }

        let backoff = RETRY_DELAYS
            .get(attempt as usize - 1)
            .copied()
            .unwrap_or(16);

        emit_status(app, logger, ConnectionStatus::Connecting, attempt, Some(err));
        sleep(Duration::from_secs(backoff)).await;
        attempt += 1;
    }
}

async fn read_loop(mut reader: tokio::net::tcp::OwnedReadHalf, tx: ConnectionQueue) {
    let mut buffer = vec![0u8; 4096];
    let mut continuation = true;

    while continuation {
        let len = reader.read(&mut buffer).await;
        let res = len.map(|l| Vec::from_iter(buffer[..l].iter().copied()));
        continuation = tx
            .send(ConnectionMessage::MessageReceived(res))
            .await
            .is_ok();
    }
}

async fn receive_loop(
    writer: OwnedWriteHalf,
    mut queue: ConnectionQueueReceiver,
    app: AppHandle,
    logger: AppLogger,
) {
    logger.log_backend(LogLevel::Inf, file!(), line!(), "receive loop started");

    let mut connection = Connection {
        app,
        writer,
        logger: logger.clone(),
        auto_response: AutoResponseConfig::default(),
    };

    while let Some(message) = queue.recv().await {
        match message {
            ConnectionMessage::Disconnect => {
                logger.log_backend(LogLevel::Inf, file!(), line!(), "disconnect message received in queue");
                connection.disconnect().await;
                queue.close();
            }
            ConnectionMessage::SendMessage(send_request) => {
                connection.send_user_message(&send_request).await;
            }
            ConnectionMessage::SetAutoResponse(auto_response_config) => {
                connection.set_auto_response(auto_response_config);
            }
            ConnectionMessage::MessageReceived(message_res) => {
                if let Err(err)  = connection.handle_message(message_res).await {
                    logger.log_backend(
                        LogLevel::Err,
                        file!(),
                        line!(),
                        format!("message handling failed: {err}"),
                    );
                    connection.disconnect_with_error(err).await;
                    queue.close();
                }
            }
        }
    }

    logger.log_backend(LogLevel::Inf, file!(), line!(), "receive loop terminated");
}

struct Connection {
    app: AppHandle,
    writer: OwnedWriteHalf,
    logger: AppLogger,
    auto_response: AutoResponseConfig,
}

impl Connection {
    pub async fn send_user_message(&mut self, payload: &SendRequest) {
        let message_bytes = translate::to_bytes(&payload.message);
        self.logger.log_backend(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("sending user message bytes={}", message_bytes.len()),
        );

        match self.writer.write_all(&message_bytes).await {
            Ok(_) => emit_message(
                &self.app,
                &self.logger,
                MessagePayload {
                    direction: MessageDirection::Sent,
                    content: payload.message.clone(),
                    timestamp: now_ts(),
                },
            ),
            Err(err) => {
                self.logger.log_backend(
                    LogLevel::Err,
                    file!(),
                    line!(),
                    format!("socket write failed bytes={}: {}", message_bytes.len(), err),
                );
                emit_status(
                    &self.app,
                    &self.logger,
                    ConnectionStatus::Error,
                    0,
                    Some(err.to_string()),
                )
            }
        }
    }

    pub async fn disconnect(&mut self) {
        match self.writer.shutdown().await {
            Ok(_) => {
                self.logger
                    .log_backend(LogLevel::Inf, file!(), line!(), "connection shutdown succeeded");
                emit_status(
                    &self.app,
                    &self.logger,
                    ConnectionStatus::Disconnected,
                    0,
                    None,
                )
            }
            Err(err) => {
                self.logger.log_backend(
                    LogLevel::Err,
                    file!(),
                    line!(),
                    format!("connection shutdown failed: {err}"),
                );
                emit_status(
                    &self.app,
                    &self.logger,
                    ConnectionStatus::Error,
                    0,
                    Some(err.to_string()),
                )
            }
        }
    }

    pub async fn disconnect_with_error(&mut self, error_message: String) {
        let _ = self.writer.shutdown().await;
        self.logger.log_backend(
            LogLevel::Err,
            file!(),
            line!(),
            format!("disconnecting due to error: {error_message}"),
        );
        emit_status(
            &self.app,
            &self.logger,
            ConnectionStatus::Error,
            0,
            Some(error_message),
        );
    }

    pub fn set_auto_response(&mut self, cfg: AutoResponseConfig) {
        self.logger.log_backend(
            LogLevel::Inf,
            file!(),
            line!(),
            format!(
                "auto-response config updated enabled={} protocol={:?}",
                cfg.enabled, cfg.protocol
            ),
        );
        self.auto_response = cfg;
    }

    pub async fn handle_message(
        &mut self,
        message: Result<Vec<u8>, io::Error>,
    ) -> Result<(), String> {
        match message {
            Ok(a) if a.is_empty() => Err("Connection closed by peer".to_string()),
            Ok(msg) => {
                self.logger.log_backend(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("received message bytes={}", msg.len()),
                );
                let visible = translate::to_human_readable(&msg);

                emit_message(
                    &self.app,
                    &self.logger,
                    MessagePayload {
                        direction: MessageDirection::Received,
                        content: visible.clone(),
                        timestamp: now_ts(),
                    },
                );

                self.send_auto_response(&msg).await;
                Ok(())
            }
            Err(err) => Err(err.to_string()),
        }
    }

    async fn send_auto_response(&mut self, message: &[u8]) {
        if let Some(response) = auto_response::build_auto_response(&self.auto_response, message) {
            self.logger.log_backend(
                LogLevel::Inf,
                file!(),
                line!(),
                format!("sending auto-response bytes={}", response.as_bytes().len()),
            );
            self.send_user_message(&SendRequest {
                message: response.clone(),
            })
            .await;
        }
    }
}

fn emit_status(
    app: &AppHandle,
    logger: &AppLogger,
    status: ConnectionStatus,
    attempts: u32,
    message: Option<String>,
) {
    let payload = StatusPayload {
        status,
        attempts,
        message,
    };

    if let Err(err) = app.emit(STATUS_EVENT, payload) {
        logger.log_backend(
            LogLevel::Err,
            file!(),
            line!(),
            format!("failed to emit status event: {err}"),
        );
    }
}

fn emit_message(app: &AppHandle, logger: &AppLogger, payload: MessagePayload) {
    if let Err(err) = app.emit(MESSAGE_EVENT, payload) {
        logger.log_backend(
            LogLevel::Err,
            file!(),
            line!(),
            format!("failed to emit message event: {err}"),
        );
    }
}

fn now_ts() -> String {
    Utc::now().to_rfc3339()
}
