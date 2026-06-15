use chrono::Utc;
use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use tauri::async_runtime::{spawn, Receiver, Sender};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::error::SendError;
use tokio::time::{sleep, timeout, Duration};

use crate::auto_response;
use crate::logger::AppLogger;
use crate::models::{
    AutoResponseConfig, Command, ConnectRequest, ConnectionStatus, LogLevel, MessageDirection,
    MessagePayload, SendRequest, StatusPayload, MESSAGE_EVENT, STATUS_EVENT,
};
use crate::translate::{self, ControlToken};

pub struct ConnectionManager {
    app: AppHandle,
    logger: AppLogger,
    segments_awaiting_ack: VecDeque<Vec<u8>>,
    auto_response: AutoResponseConfig,
    message_sender: Option<Sender<Vec<u8>>>,
    disconnect_sender: Option<Sender<()>>,
}

impl ConnectionManager {
    pub fn new(app: AppHandle, logger: AppLogger) -> Self {
        Self {
            app: app,
            logger: logger,
            segments_awaiting_ack: VecDeque::new(),
            auto_response: AutoResponseConfig::default(),
            message_sender: None,
            disconnect_sender: None,
        }
    }

    pub fn connect(&mut self, req: ConnectRequest, command_sender: Sender<Command>) -> () {
        if self.message_sender.is_some() {
            self.logger.log_backend(
                LogLevel::Wrn,
                file!(),
                line!(),
                "connect message received while connection already active or connecting, ignoring",
            );
            emit_status(&self.app, &self.logger, ConnectionStatus::Connected);
            return;
        }

        let address: SocketAddr = match format!("{}:{}", req.ip, req.port).parse() {
            Ok(addr) => addr,
            Err(err) => {
                emit_status(&self.app, &self.logger, ConnectionStatus::Error);
                self.logger.log_backend(
                    LogLevel::Err,
                    file!(),
                    line!(),
                    format!("invalid socket address for connect message: {err}"),
                );
                return;
            }
        };

        let (tx, rx) = tauri::async_runtime::channel::<Vec<u8>>(10);
        let (tx_dc, rx_dc) = tauri::async_runtime::channel(1);
        spawn(connect_and_read(
            self.app.clone(),
            self.logger.clone(),
            address,
            command_sender,
            rx,
            rx_dc,
        ));

        self.message_sender = Some(tx);
        self.disconnect_sender = Some(tx_dc);
    }

    pub async fn send_user_message(&mut self, payload: &SendRequest) {
        self.logger.log_backend(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("sending user message={}", payload.message),
        );

        let mut lines = payload.message.lines().map(translate::to_bytes);

        while let Some(line) = lines.next() {
            let requires_ack = requires_ack(&line);

            if let Err(_) = self.send_message(&line).await {
                return;
            }

            if requires_ack {
                self.segments_awaiting_ack.extend(lines);
                break;
            }
        }
    }

    async fn send_message(&mut self, message: &[u8]) -> Result<(), String> {
        self.logger.log_backend(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("sending user message bytes={}", message.len()),
        );
        match &self.message_sender {
            None => {
                emit_message(
                    &self.app,
                    &self.logger,
                    MessagePayload {
                        direction: MessageDirection::System,
                        content: "ERROR: Could not send the message, are you connected ?"
                            .to_string(),
                        timestamp: now_ts(),
                    },
                );
                Ok(())
            }
            Some(sender) => match sender.send(message.to_vec()).await {
                Err(err) => {
                    let err: SendError<Vec<u8>> = err;
                    self.logger.log_backend(
                        LogLevel::Err,
                        file!(),
                        line!(),
                        format!("socket write failed bytes={}: {}", message.len(), err),
                    );
                    emit_status(&self.app, &self.logger, ConnectionStatus::Error);
                    Err(err.to_string())
                }
                Ok(_) => Ok(()),
            },
        }
    }

    pub async fn disconnect(&mut self) {
        self.logger.log_backend(
            LogLevel::Inf,
            file!(),
            line!(),
            "disconnect message received in queue",
        );

        match &mut self.disconnect_sender {
            None => {
                self.logger.log_backend(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    "no connection, disconnect successful",
                );
                emit_status(&self.app, &self.logger, ConnectionStatus::Disconnected);
            }
            Some(sender) => match sender.send(()).await {
                Ok(()) => (),
                Err(_) => {
                    self.logger.log_backend(
                        LogLevel::Err,
                        file!(),
                        line!(),
                        "error while disconnecting",
                    );
                    emit_status(&self.app, &self.logger, ConnectionStatus::Error);
                }
            },
        }
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

                if let Some(next_segment) = self.segments_awaiting_ack.pop_front() {
                    self.logger.log_backend(
                        LogLevel::Inf,
                        file!(),
                        line!(),
                        "received expected ACK, sending next segment",
                    );
                    sleep(Duration::from_millis(200)).await;
                    self.send_message(&next_segment).await?;
                }
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

async fn connect_and_read(
    app: AppHandle,
    logger: AppLogger,
    addr: SocketAddr,
    message_sender: Sender<Command>,
    message_receiver: Receiver<Vec<u8>>,
    mut dc_receiver: Receiver<()>,
) -> () {
    emit_status(&app, &logger, ConnectionStatus::Connecting);

    let mut stream: Option<TcpStream> = None;
    tokio::select! {
        res = loop_till_connect(&app, &logger, addr) => {
            match res {
                Ok(tcp_stream) => { stream = Some(tcp_stream); },
                Err(_) => { return (); },
            }
        }
        _ = dc_receiver.recv() => {
            emit_status(&app, &logger, ConnectionStatus::Disconnected);
            logger.log_backend(
                LogLevel::Inf,
                file!(),
                line!(),
                "connect attempt interrupted",
            );
            return ();
        }
    }

    let (mut reader, mut writer) = stream.unwrap().into_split();
    emit_status(&app, &logger, ConnectionStatus::Connected);

    tokio::select! {
        _ = read_loop(&mut reader, message_sender) => {}
        _ = send_loop(&app, &logger, &mut writer, message_receiver) => {
            emit_status(&app, &logger, ConnectionStatus::Error);
            logger.log_backend(
                LogLevel::Inf,
                file!(),
                line!(),
                "an error occurred while writing, disconnecting",
            );
        }
        _ = dc_receiver.recv() => {
            emit_status(&app, &logger, ConnectionStatus::Disconnected);
            logger.log_backend(
                LogLevel::Inf,
                file!(),
                line!(),
                "disconnected successfully",
            );
        }
    }

    let _ = writer.shutdown().await;
}

async fn loop_till_connect(
    app: &AppHandle,
    logger: &AppLogger,
    addr: SocketAddr,
) -> Result<TcpStream, ()> {
    let mut attempt = 1;
    loop {
        logger.log_backend(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("connect attempt={} address={}", attempt, addr),
        );

        let result = timeout(Duration::from_secs(1), TcpStream::connect(addr)).await;

        match result {
            Ok(Ok(tcp_stream)) => {
                emit_status(app, logger, ConnectionStatus::Connected);
                logger.log_backend(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("connect succeeded attempt={} address={}", attempt, addr),
                );
                return Ok(tcp_stream);
            }
            Ok(Err(err)) => {
                logger.log_backend(
                    LogLevel::Err,
                    file!(),
                    line!(),
                    format!(
                        "connect failed attempt={} address={} error={}",
                        attempt,
                        addr,
                        err.to_string()
                    ),
                );
                emit_status(&app, logger, ConnectionStatus::Error);
                return Err(());
            }
            Err(_err) => {
                logger.log_backend(
                    LogLevel::Wrn,
                    file!(),
                    line!(),
                    format!(
                        "connect failed attempt={} address={}: time elapsed",
                        attempt, addr
                    ),
                );
            }
        };

        sleep(Duration::from_secs(1)).await;
        attempt += 1;
    }
}

async fn read_loop(reader: &mut tokio::net::tcp::OwnedReadHalf, tx: Sender<Command>) {
    let mut buffer = vec![0u8; 4096];
    let mut continuation = true;

    while continuation {
        let len = reader.read(&mut buffer).await;
        let res = len.map(|l| Vec::from_iter(buffer[..l].iter().copied()));
        continuation = tx.send(Command::MessageReceived(res)).await.is_ok();
    }
}

async fn send_loop(
    app: &AppHandle,
    logger: &AppLogger,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    mut rx: Receiver<Vec<u8>>,
) {
    while let Some(msg) = rx.recv().await {
        let mut pos = 0;

        while pos < msg.len() {
            let result = writer.write(&msg[pos..]).await;
            match result {
                Ok(0) => return,
                Ok(i) => pos += i,
                Err(_) => return,
            }
        }

        emit_message(
            app,
            logger,
            MessagePayload {
                direction: MessageDirection::Sent,
                content: translate::to_human_readable(&msg),
                timestamp: now_ts(),
            },
        );
    }
}

fn emit_status(app: &AppHandle, logger: &AppLogger, status: ConnectionStatus) {
    let payload = StatusPayload { status };

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

fn requires_ack(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }

    line[0] == ControlToken::ENQ as u8 || line[0] == ControlToken::STX as u8
}
