use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter as _};
use tauri_plugin_notification::NotificationExt;

use crate::{
    logger::AppLogger,
    models::{
        ConnectionStatus, FrontendLogEntry, LogLevel, MessagePayload, MessageType, StatusPayload,
        MESSAGE_EVENT, STATUS_EVENT,
    },
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

/// Identifies who produced an event: a session and one of its connection attempts.
#[derive(Clone, Debug)]
struct Scope {
    session_id: String,
    attempt: u64,
}

/// Root emitters (no scope) only write logs: events always belong to a session,
/// so a global log can never pollute whichever conversation happens to be selected.
#[derive(Clone)]
pub struct Emitter {
    sink: Arc<dyn EventSink>,
    logger: Option<AppLogger>,
    scope: Option<Scope>,
}

impl Emitter {
    pub fn new(app: AppHandle, logger: AppLogger) -> Self {
        Self {
            sink: Arc::new(TauriSink::new(app)),
            logger: Some(logger),
            scope: None,
        }
    }

    /// Root emitter writing to an arbitrary sink, without a file logger.
    #[cfg(test)]
    pub fn with_sink(sink: Arc<dyn EventSink>) -> Self {
        Self {
            sink,
            logger: None,
            scope: None,
        }
    }

    /// Emitter whose events carry `session_id` and `attempt`.
    pub fn scoped(&self, session_id: &str, attempt: u64) -> Self {
        Self {
            sink: self.sink.clone(),
            logger: self.logger.clone(),
            scope: Some(Scope {
                session_id: session_id.to_string(),
                attempt,
            }),
        }
    }

    fn log(&self, level: LogLevel, file: &str, line: u32, message: String) {
        if let Some(logger) = &self.logger {
            match &self.scope {
                Some(scope) => logger.log_backend(
                    level,
                    file,
                    line,
                    format!(
                        "[session={} attempt={}] {}",
                        scope.session_id, scope.attempt, message
                    ),
                ),
                None => logger.log_backend(level, file, line, message),
            }
        }
    }

    pub fn info(&self, file: &str, line: u32, message: impl ToString) {
        self.log(LogLevel::Inf, file, line, message.to_string());
        self.emit_message(MessageType::SystemInfo, message.to_string())
    }

    pub fn warn(&self, file: &str, line: u32, message: impl ToString) {
        self.log(LogLevel::Wrn, file, line, message.to_string());
        self.emit_message(MessageType::SystemWarn, message.to_string())
    }

    pub fn error(&self, file: &str, line: u32, message: impl ToString) {
        self.log(LogLevel::Err, file, line, message.to_string());
        self.emit_message(MessageType::SystemError, message.to_string())
    }

    pub fn only_log(&self, level: LogLevel, file: &str, line: u32, message: impl ToString) {
        self.log(level, file, line, message.to_string());
    }

    pub fn log_frontend(&self, entry: &FrontendLogEntry) {
        if let Some(logger) = &self.logger {
            logger.log_frontend(entry);
        }
    }

    pub fn emit_status(&self, status: ConnectionStatus) {
        let Some(scope) = &self.scope else {
            return;
        };
        let payload = StatusPayload {
            session_id: scope.session_id.clone(),
            attempt: scope.attempt,
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

    pub fn emit_message(&self, msg_type: MessageType, content: String) {
        self.emit_payload(msg_type, content, None);
    }

    /// Emits a Sent/Received event that also carries the exact wire bytes.
    pub fn emit_message_with_raw(&self, msg_type: MessageType, content: String, raw: Vec<u8>) {
        self.emit_payload(msg_type, content, Some(raw));
    }

    fn emit_payload(&self, msg_type: MessageType, content: String, raw: Option<Vec<u8>>) {
        let Some(scope) = &self.scope else {
            return;
        };
        let payload = MessagePayload {
            session_id: scope.session_id.clone(),
            attempt: scope.attempt,
            content,
            msg_type,
            timestamp: now_ts(),
            raw,
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
