//! Request DTOs sent by the frontend (wire shapes, not yet validated) and their responses.

use crate::protocol::astm::AstmEndingMode;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub message: String,
}

/// (De)serializes an `AstmEndingMode` as the boolean `no_etb`.
mod no_etb_flag {
    use super::AstmEndingMode;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(mode: &AstmEndingMode, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(mode.no_etb())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<AstmEndingMode, D::Error> {
        bool::deserialize(d).map(AstmEndingMode::from_no_etb)
    }
}

/// Wire shape: `{"input": "...", "no_etb": bool}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoBuildRequest {
    pub input: String,
    #[serde(rename = "no_etb", with = "no_etb_flag")]
    pub astm_ending: AstmEndingMode,
}
