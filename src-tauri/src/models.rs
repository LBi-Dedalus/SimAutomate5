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
#[serde(tag = "type")]
pub enum ConnectRequest {
    ClientConnectRequest { ip: String, port: u16 },
    ServerStartRequest { port: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoBuildRequest {
    pub input: String,
    pub no_etb: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResponse {
    pub output: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_status_serializes_lowercase() {
        let json = serde_json::to_string(&StatusPayload {
            session_id: "s1".to_string(),
            attempt: 2,
            status: ConnectionStatus::Listening,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"session_id":"s1","attempt":2,"status":"listening"}"#
        );
    }

    #[test]
    fn message_payload_omits_absent_raw_and_keeps_present_raw() {
        let mut payload = MessagePayload {
            session_id: "s1".to_string(),
            attempt: 1,
            msg_type: MessageType::SystemInfo,
            content: "hi".to_string(),
            timestamp: "t".to_string(),
            raw: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("raw"));
        assert!(json.contains(r#""session_id":"s1""#));
        assert!(json.contains(r#""attempt":1"#));

        payload.raw = Some(vec![0x0b, 0x41]);
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains(r#""raw":[11,65]"#));
    }
}
