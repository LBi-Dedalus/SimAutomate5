use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter as _};
use tauri_plugin_notification::NotificationExt;

use super::Logger;
use crate::{
    ids::ConnectionId,
    models::{
        ConnectionStatus, LogLevel, MessagePayload, MessageType, StatusPayload, MESSAGE_EVENT,
        STATUS_EVENT,
    },
    protocol::astm::AstmUnit,
};

/// Destination of the events produced by the connections.
/// The application uses Tauri; tests use an in-memory sink.
pub trait EventSink: Send + Sync {
    fn status(&self, payload: StatusPayload) -> Result<(), String>;
    fn message(&self, payload: MessagePayload) -> Result<(), String>;
    fn notify(&self, title: &str, body: &str) -> Result<(), String>;
}

pub struct TauriSink {
    app: AppHandle,
}

impl TauriSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriSink {
    fn status(&self, payload: StatusPayload) -> Result<(), String> {
        self.app
            .emit(STATUS_EVENT, payload)
            .map_err(|err| err.to_string())
    }

    fn message(&self, payload: MessagePayload) -> Result<(), String> {
        self.app
            .emit(MESSAGE_EVENT, payload)
            .map_err(|err| err.to_string())
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), String> {
        self.app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|err| err.to_string())
    }
}

/// Builds the reporter of each connection from the shared sink and logger.
#[derive(Clone)]
pub struct ConnectionReporterFactory {
    sink: Arc<dyn EventSink>,
    logger: Arc<dyn Logger>,
}

impl ConnectionReporterFactory {
    pub fn new(sink: Arc<dyn EventSink>, logger: Arc<dyn Logger>) -> Self {
        Self { sink, logger }
    }

    pub fn create(&self, connection: &ConnectionId) -> ConnectionReporter {
        ConnectionReporter {
            sink: self.sink.clone(),
            logger: self.logger.clone(),
            connection: connection.clone(),
        }
    }
}

/// Logs and emits events on behalf of one connection: events always belong to a session,
/// so a global log can never pollute whichever conversation happens to be selected.
#[derive(Clone)]
pub struct ConnectionReporter {
    sink: Arc<dyn EventSink>,
    logger: Arc<dyn Logger>,
    connection: ConnectionId,
}

impl ConnectionReporter {
    fn log(&self, level: LogLevel, file: &str, line: u32, message: String) {
        self.logger.log_backend(
            level,
            file,
            line,
            &format!("[{}] {message}", self.connection),
        );
    }

    pub fn info(&self, file: &str, line: u32, message: impl ToString) {
        self.log(LogLevel::Inf, file, line, message.to_string());
        self.emit_payload(MessageType::SystemInfo, message.to_string(), None, None)
    }

    pub fn warn(&self, file: &str, line: u32, message: impl ToString) {
        self.log(LogLevel::Wrn, file, line, message.to_string());
        self.emit_payload(MessageType::SystemWarn, message.to_string(), None, None)
    }

    pub fn error(&self, file: &str, line: u32, message: impl ToString) {
        self.log(LogLevel::Err, file, line, message.to_string());
        self.emit_payload(MessageType::SystemError, message.to_string(), None, None)
    }

    pub fn only_log(&self, level: LogLevel, file: &str, line: u32, message: impl ToString) {
        self.log(level, file, line, message.to_string());
    }

    pub fn emit_status(&self, status: ConnectionStatus) {
        let connection = &self.connection;
        let payload = StatusPayload {
            session_id: connection.session_id.to_string(),
            attempt: connection.attempt.get(),
            status,
        };

        if let Err(err) = self.sink.status(payload) {
            self.log(
                LogLevel::Err,
                file!(),
                line!(),
                format!("failed to emit status event: {err}"),
            );
        }
    }

    /// Emits a Sent/Received event that also carries the exact wire bytes.
    pub fn emit_complete_message(&self, msg_type: MessageType, content: String, raw: Vec<u8>) {
        self.emit_payload(msg_type, content, Some(raw), None);
    }

    /// Emits a Sent/Received event standing for exactly one ASTM unit.
    pub fn emit_astm_unit(
        &self,
        msg_type: MessageType,
        content: String,
        raw: Vec<u8>,
        unit: AstmUnit,
    ) {
        self.emit_payload(msg_type, content, Some(raw), Some(unit));
    }

    fn emit_payload(
        &self,
        msg_type: MessageType,
        content: String,
        raw: Option<Vec<u8>>,
        astm: Option<AstmUnit>,
    ) {
        let connection = &self.connection;
        let payload = MessagePayload {
            session_id: connection.session_id.to_string(),
            attempt: connection.attempt.get(),
            content,
            msg_type,
            timestamp: now_ts(),
            raw,
            astm,
        };
        if let Err(err) = self.sink.message(payload) {
            self.log(
                LogLevel::Err,
                file!(),
                line!(),
                format!("failed to emit message event: {err}"),
            );
        }
    }

    /// A failing desktop notification must never abort a connection task.
    pub fn emit_notification(&self, title: &str, body: &str) {
        if let Err(err) = self.sink.notify(title, body) {
            self.log(
                LogLevel::Wrn,
                file!(),
                line!(),
                format!("failed to show notification: {err}"),
            );
        }
    }
}

fn now_ts() -> String {
    Utc::now().to_rfc3339()
}
