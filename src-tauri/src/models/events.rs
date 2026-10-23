//! Event names, event payloads and log types emitted to / received from the frontend.

use serde::{Deserialize, Serialize};

pub const STATUS_EVENT: &str = "connection://status";
pub const MESSAGE_EVENT: &str = "message://stream";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogLevel {
    #[serde(rename = "INF")]
    Inf,
    #[serde(rename = "WRN")]
    Wrn,
    #[serde(rename = "ERR")]
    Err,
}

impl LogLevel {
    pub const fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Inf => "INF",
            LogLevel::Wrn => "WRN",
            LogLevel::Err => "ERR",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendLogEntry {
    pub level: LogLevel,
    pub location: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessagePayload {
    /// Session that produced the event; the frontend must never guess it.
    pub session_id: String,
    /// Connection attempt of the session that produced the event.
    pub attempt: u64,
    pub msg_type: MessageType,
    pub content: String,
    pub timestamp: String,
    /// Exact wire bytes of a Sent/Received event, when known. Absent for system events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    Sent,
    Received,
    SystemInfo,
    SystemWarn,
    SystemError,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusPayload {
    pub session_id: String,
    pub attempt: u64,
    pub status: ConnectionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    /// Server mode only: the port is bound and the server waits for a client.
    Listening,
    Connected,
    Error,
}
