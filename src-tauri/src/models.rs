use serde::{Deserialize, Serialize};

pub const STATUS_EVENT: &str = "connection://status";
pub const MESSAGE_EVENT: &str = "message://stream";
pub const MAX_RETRIES: u32 = 6;
pub const RETRY_DELAYS: [u64; 6] = [1, 2, 4, 8, 16, 32];

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Astm,
    Hl7,
}

impl Default for Protocol {
    fn default() -> Self {
        Protocol::Astm
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectRequest {
    pub ip: String,
    pub port: u16,
    pub retries_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoResponseConfig {
    pub enabled: bool,
    pub protocol: Protocol,
    pub astm_message: Option<String>,
    pub hl7_message_type: Option<String>,
    pub hl7_response_code: Option<String>,
}

impl AutoResponseConfig {
    pub const fn default() -> Self {
        AutoResponseConfig {
            enabled: false,
            protocol: Protocol::Astm,
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
    pub direction: MessageDirection,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageDirection {
    Sent,
    Received,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusPayload {
    pub status: ConnectionStatus,
    pub attempts: u32,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
}
