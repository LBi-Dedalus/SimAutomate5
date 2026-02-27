use serde::{Deserialize, Serialize};

pub const STATUS_EVENT: &str = "connection://status";
pub const MESSAGE_EVENT: &str = "message://stream";
pub const MAX_RETRIES: u32 = 5;
pub const RETRY_DELAYS: [u64; 5] = [1, 2, 4, 8, 16];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Astm,
    Hl7,
    Mllp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectRequest {
    pub ip: String,
    pub port: u16,
    pub protocol: Protocol,
    pub retries_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AutoResponseConfig {
    pub enabled: bool,
    pub astm_message: Option<String>,
    pub hl7_message_type: Option<String>,
    pub hl7_response_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRequest {
    pub protocol: Protocol,
    pub input: String,
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
    pub protocol: Protocol,
    pub content: String,
    pub timestamp: String,
    pub auto_response: bool,
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
