//! Request DTOs sent by the frontend (wire shapes, not yet validated) and their responses.

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

/// How intermediate ASTM records end. The last record always ends with ETX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstmEndingMode {
    /// Every record but the last ends with ETB (the standard behaviour).
    EtbBetweenRecords,
    /// Every record ends with ETX ("No ETB").
    EtxForEveryRecord,
}

impl AstmEndingMode {
    /// From the wire `no_etb` boolean.
    pub fn from_no_etb(no_etb: bool) -> Self {
        if no_etb {
            Self::EtxForEveryRecord
        } else {
            Self::EtbBetweenRecords
        }
    }

    /// To the wire `no_etb` boolean.
    pub fn no_etb(self) -> bool {
        self == Self::EtxForEveryRecord
    }
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

#[derive(Debug, Clone, Serialize)]
pub struct BuildResponse {
    pub output: String,
}
