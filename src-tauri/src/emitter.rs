use chrono::Utc;
use tauri::{AppHandle, Emitter as _, Manager, State};
use tokio::sync::Mutex;

use crate::{
    app_state::AppState,
    logger::AppLogger,
    models::{
        ConnectionStatus, FrontendLogEntry, LogLevel, MessagePayload, MessageType, StatusPayload,
        MESSAGE_EVENT, STATUS_EVENT,
    },
};

#[derive(Clone)]
pub struct Emitter {
    app: AppHandle,
    logger: AppLogger,
}

impl Emitter {
    pub fn new(app: AppHandle, logger: AppLogger) -> Self {
        return Self { app, logger };
    }

    pub fn info(&self, file: &str, line: u32, message: impl ToString) {
        self.logger
            .log_backend(LogLevel::Inf, file, line, message.to_string());
        self.emit_message(MessageType::SystemInfo, message.to_string())
    }

    pub fn warn(&self, file: &str, line: u32, message: impl ToString) {
        self.logger
            .log_backend(LogLevel::Wrn, file, line, message.to_string());
        self.emit_message(MessageType::SystemWarn, message.to_string())
    }

    pub fn error(&self, file: &str, line: u32, message: impl ToString) {
        self.logger
            .log_backend(LogLevel::Err, file, line, message.to_string());
        self.emit_message(MessageType::SystemError, message.to_string())
    }

    pub fn only_log(&self, level: LogLevel, file: &str, line: u32, message: impl ToString) {
        self.logger
            .log_backend(level, file, line, message.to_string());
    }

    pub fn log_frontend(&self, entry: &FrontendLogEntry) {
        self.logger.log_frontend(entry);
    }

    pub fn emit_status(&self, status: ConnectionStatus) {
        let payload = StatusPayload { status };

        if let Err(err) = self.app.emit(STATUS_EVENT, payload) {
            self.logger.log_backend(
                LogLevel::Err,
                file!(),
                line!(),
                format!("failed to emit status event: {err}"),
            );
        }
    }

    pub fn emit_message(&self, msg_type: MessageType, content: String) {
        if let Err(err) = self.app.emit(
            MESSAGE_EVENT,
            MessagePayload {
                content: content,
                msg_type: msg_type,
                timestamp: now_ts(),
            },
        ) {
            self.logger.log_backend(
                LogLevel::Err,
                file!(),
                line!(),
                format!("failed to emit message event: {err}"),
            );
        }
    }

    pub async fn emit_disconnect(&self) {
        let state: State<'_, Mutex<AppState>> = self.app.state();
        let mut appstate = state.lock().await;
        appstate.connection_manager.shutdown_now();
    }
}

fn now_ts() -> String {
    Utc::now().to_rfc3339()
}
