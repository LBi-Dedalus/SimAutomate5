use std::num::NonZeroU16;

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
    ClientConnectRequest {
        ip: String,
        port: u16,
    },
    ServerStartRequest {
        port: u16,
    },
    /// Like the client mode, through an SSH tunnel; `ip`/`port` are the target as seen from the
    /// SSH host. `ssh_user` may be empty.
    SshClientConnectRequest {
        ip: String,
        port: u16,
        ssh_host: String,
        ssh_port: u16,
        ssh_user: String,
    },
    /// Like the server mode, listening on the SSH host. `ssh_user` may be empty.
    SshServerStartRequest {
        port: u16,
        ssh_host: String,
        ssh_port: u16,
        ssh_user: String,
    },
}

/// A TCP port, 1 to 65535.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port(NonZeroU16);

impl Port {
    /// `name` ("port", "SSH port") only appears in the error message.
    fn parse(raw: u16, name: &str) -> Result<Self, String> {
        NonZeroU16::new(raw)
            .map(Self)
            .ok_or_else(|| format!("The {name} must be between 1 and 65535"))
    }

    pub fn get(self) -> u16 {
        self.0.get()
    }
}

/// A host name or address, trimmed and not empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host(String);

impl Host {
    /// `name` ("host", "SSH host") only appears in the error message.
    fn parse(raw: &str, name: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(format!("The {name} is required"));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An SSH user name, trimmed and not empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User(String);

impl User {
    /// A blank name means "not given".
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The SSH server the connection goes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTunnel {
    pub host: Host,
    pub port: Port,
    /// `None`: resolved like OpenSSH does (SSH config, then the local user name).
    pub user: Option<User>,
}

impl SshTunnel {
    fn parse(host: &str, port: u16, user: &str) -> Result<Self, String> {
        Ok(Self {
            host: Host::parse(host, "SSH host")?,
            port: Port::parse(port, "SSH port")?,
            user: User::parse(user),
        })
    }
}

/// A `ConnectRequest` whose fields were checked: built only by `TryFrom<ConnectRequest>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionSpec {
    Client {
        host: Host,
        port: Port,
    },
    Server {
        port: Port,
    },
    SshClient {
        host: Host,
        port: Port,
        tunnel: SshTunnel,
    },
    SshServer {
        port: Port,
        tunnel: SshTunnel,
    },
}

impl TryFrom<ConnectRequest> for ConnectionSpec {
    type Error = String;

    fn try_from(req: ConnectRequest) -> Result<Self, String> {
        match req {
            ConnectRequest::ClientConnectRequest { ip, port } => Ok(Self::Client {
                host: Host::parse(&ip, "host")?,
                port: Port::parse(port, "port")?,
            }),
            ConnectRequest::ServerStartRequest { port } => Ok(Self::Server {
                port: Port::parse(port, "port")?,
            }),
            ConnectRequest::SshClientConnectRequest {
                ip,
                port,
                ssh_host,
                ssh_port,
                ssh_user,
            } => Ok(Self::SshClient {
                host: Host::parse(&ip, "host")?,
                port: Port::parse(port, "port")?,
                tunnel: SshTunnel::parse(&ssh_host, ssh_port, &ssh_user)?,
            }),
            ConnectRequest::SshServerStartRequest {
                port,
                ssh_host,
                ssh_port,
                ssh_user,
            } => Ok(Self::SshServer {
                port: Port::parse(port, "port")?,
                tunnel: SshTunnel::parse(&ssh_host, ssh_port, &ssh_user)?,
            }),
        }
    }
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

    fn spec(json: &str) -> Result<ConnectionSpec, String> {
        let req: ConnectRequest = serde_json::from_str(json).unwrap();
        ConnectionSpec::try_from(req)
    }

    #[test]
    fn client_and_server_requests_are_parsed() {
        let spec_client = spec(r#"{"type":"ClientConnectRequest","ip":"  10.0.0.1 ","port":5000}"#);
        let ConnectionSpec::Client { host, port } = spec_client.unwrap() else {
            panic!("expected a client");
        };
        assert_eq!(host.as_str(), "10.0.0.1");
        assert_eq!(port.get(), 5000);

        let spec_server = spec(r#"{"type":"ServerStartRequest","port":65535}"#).unwrap();
        assert!(matches!(spec_server, ConnectionSpec::Server { port } if port.get() == 65535));
    }

    #[test]
    fn invalid_client_and_server_requests_keep_their_messages() {
        assert_eq!(
            spec(r#"{"type":"ClientConnectRequest","ip":"  ","port":1}"#).unwrap_err(),
            "The host is required"
        );
        assert_eq!(
            spec(r#"{"type":"ClientConnectRequest","ip":"h","port":0}"#).unwrap_err(),
            "The port must be between 1 and 65535"
        );
        assert_eq!(
            spec(r#"{"type":"ServerStartRequest","port":0}"#).unwrap_err(),
            "The port must be between 1 and 65535"
        );
    }

    #[test]
    fn ssh_client_request_is_parsed_with_its_tunnel() {
        let parsed = spec(
            r#"{"type":"SshClientConnectRequest","ip":"127.0.0.1","port":9000,
                "ssh_host":" vm ","ssh_port":22,"ssh_user":" bob "}"#,
        )
        .unwrap();
        let ConnectionSpec::SshClient { host, port, tunnel } = parsed else {
            panic!("expected an SSH client");
        };
        assert_eq!(host.as_str(), "127.0.0.1");
        assert_eq!(port.get(), 9000);
        assert_eq!(tunnel.host.as_str(), "vm");
        assert_eq!(tunnel.port.get(), 22);
        assert_eq!(tunnel.user.unwrap().as_str(), "bob");
    }

    #[test]
    fn ssh_server_request_is_parsed_and_an_empty_user_is_none() {
        for user in ["", "   "] {
            let json = format!(
                r#"{{"type":"SshServerStartRequest","port":9000,"ssh_host":"vm","ssh_port":2222,"ssh_user":"{user}"}}"#
            );
            let ConnectionSpec::SshServer { port, tunnel } = spec(&json).unwrap() else {
                panic!("expected an SSH server");
            };
            assert_eq!(port.get(), 9000);
            assert_eq!(tunnel.port.get(), 2222);
            assert_eq!(tunnel.user, None);
        }
    }

    #[test]
    fn invalid_ssh_requests_are_rejected() {
        let client = |ip: &str, port: u16, host: &str, ssh_port: u16| {
            spec(&format!(
                r#"{{"type":"SshClientConnectRequest","ip":"{ip}","port":{port},"ssh_host":"{host}","ssh_port":{ssh_port},"ssh_user":""}}"#
            ))
        };
        assert!(client("h", 1, "vm", 22).is_ok());
        assert_eq!(
            client(" ", 1, "vm", 22).unwrap_err(),
            "The host is required"
        );
        assert_eq!(
            client("h", 0, "vm", 22).unwrap_err(),
            "The port must be between 1 and 65535"
        );
        assert_eq!(
            client("h", 1, " ", 22).unwrap_err(),
            "The SSH host is required"
        );
        assert_eq!(
            client("h", 1, "vm", 0).unwrap_err(),
            "The SSH port must be between 1 and 65535"
        );

        assert_eq!(
            spec(
                r#"{"type":"SshServerStartRequest","port":0,"ssh_host":"vm","ssh_port":22,"ssh_user":""}"#
            )
            .unwrap_err(),
            "The port must be between 1 and 65535"
        );
        assert_eq!(
            spec(
                r#"{"type":"SshServerStartRequest","port":1,"ssh_host":"","ssh_port":22,"ssh_user":""}"#
            )
            .unwrap_err(),
            "The SSH host is required"
        );
        assert_eq!(
            spec(
                r#"{"type":"SshServerStartRequest","port":1,"ssh_host":"vm","ssh_port":0,"ssh_user":""}"#
            )
            .unwrap_err(),
            "The SSH port must be between 1 and 65535"
        );
    }

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
