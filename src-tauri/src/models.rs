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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectRequest {
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoResponseConfig {
    pub enabled: bool,
    pub astm_message: Option<String>,
    pub hl7_message_type: Option<String>,
    pub hl7_response_code: Option<String>,
}

impl AutoResponseConfig {
    pub const fn default() -> Self {
        AutoResponseConfig {
            enabled: false,
            astm_message: None,
            hl7_message_type: None,
            hl7_response_code: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoBuildRequest {
    pub input: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResponse {
    pub output: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessagePayload {
    pub msg_type: MessageType,
    pub content: String,
    pub timestamp: String,
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
    pub status: ConnectionStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
}
