//! Validated connection types: built from a `ConnectRequest`, they cannot hold invalid values.

use std::num::NonZeroU16;

use super::request::ConnectRequest;

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
