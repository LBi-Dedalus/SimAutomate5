//! Wire models shared with the frontend, and validated connection types.
//!
//! `request` (frontend request DTOs), `connection` (validated types), `events` (event payloads,
//! constants and log types). Everything is re-exported here: use `crate::models::X`.

mod connection;
mod events;
mod request;

pub use connection::{ConnectionSpec, Host, Port, SshTunnel, User};
pub use events::{
    ConnectionStatus, FrontendLogEntry, LogLevel, MessagePayload, MessageType, StatusPayload,
    MESSAGE_EVENT, STATUS_EVENT,
};
pub use request::{AstmEndingMode, AutoBuildRequest, BuildResponse, ConnectRequest, SendRequest};

#[cfg(test)]
mod tests;
