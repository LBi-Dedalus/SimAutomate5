//! Session level operations behind the Tauri commands. They are plain functions over the
//! `ConnectionManager` so that the exact same code is exercised by the tests with real sockets.
//!
//! Configuration operations (templates, auto reply rules) go through the `ConfigStore`
//! (read -> validate/compile -> atomic write -> new revisioned configuration), then the result
//! is applied to the `ConnectionManager`, which ignores a configuration older than the one it
//! runs. Nothing is applied when a step fails, so a failed save keeps the previous behaviour.

use crate::auto_reply::AutoReplyConfig;
use crate::config_store::{ConfigStore, RevisionedConfig, Template};
use crate::ids::{ConnectionId, SessionId};
use crate::models::{ConnectRequest, ConnectionSpec, LogLevel, SendRequest};
use crate::reporting::Logger;
use crate::transport::ConnectionManager;

#[cfg(test)]
mod tests;

/// How `stop` ends a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopMode {
    /// The session stays registered and can be reconnected.
    Disconnect,
    /// The session is forgotten (closed and removed).
    Close,
}

pub async fn connect(
    manager: &ConnectionManager,
    session_id: &str,
    attempt: u64,
    req: ConnectRequest,
) -> Result<(), String> {
    let connection = ConnectionId::parse(session_id, attempt)?;
    let spec = ConnectionSpec::try_from(req)?;
    manager.connect(&connection, spec).await
}

/// Queues a message on the given attempt of the given session, only if it is connected.
pub async fn send(
    manager: &ConnectionManager,
    session_id: &str,
    attempt: u64,
    payload: &SendRequest,
) -> Result<(), String> {
    let connection = ConnectionId::parse(session_id, attempt)?;
    let queue = manager.send_target(&connection).await?;
    queue.send_user_message(payload).await
}

/// Stops one session: `Close` forgets it, `Disconnect` keeps it registered for a reconnect.
pub async fn stop(
    manager: &ConnectionManager,
    session_id: &str,
    mode: StopMode,
) -> Result<(), String> {
    let session_id = SessionId::parse(session_id)?;
    manager.stop(&session_id, mode == StopMode::Close).await
}

async fn apply_rules(manager: &ConnectionManager, config: RevisionedConfig) -> RevisionedConfig {
    manager.apply_rules(&config).await;
    config
}

/// Startup: loads the persisted rules and templates before any connection exists. Invalid
/// data leaves automatic replies disabled (the file is never touched) and is logged with
/// metadata only; the Rules view reports the same failure when it loads.
pub async fn init_auto_reply(
    store: &ConfigStore,
    manager: &ConnectionManager,
    logger: &dyn Logger,
) {
    match store.load().await {
        Ok(config) => {
            manager.apply_rules(&config).await;
        }
        Err(err) => logger.log_backend(
            LogLevel::Err,
            file!(),
            line!(),
            &format!("auto reply disabled, the saved configuration is invalid: {err}"),
        ),
    }
}

/// The effective templates and auto reply rules (defaults for missing keys), stamped with the
/// current configuration revision (loads do not change it).
pub async fn load_config(store: &ConfigStore) -> Result<RevisionedConfig, String> {
    store.load().await
}

/// Saves the templates (refused when a rule would break) and refreshes the running rules.
/// Every successful save bumps the revision and returns the new one, so consumers can discard
/// a response that is delivered after a newer one.
pub async fn save_templates(
    store: &ConfigStore,
    manager: &ConnectionManager,
    templates: &[Template],
) -> Result<RevisionedConfig, String> {
    let config = store.save_templates(templates).await?;
    Ok(apply_rules(manager, config).await)
}

pub async fn save_auto_reply(
    store: &ConfigStore,
    manager: &ConnectionManager,
    config: &AutoReplyConfig,
) -> Result<RevisionedConfig, String> {
    let config = store.save_auto_reply(config).await?;
    Ok(apply_rules(manager, config).await)
}

/// Persists and applies the master switch only; the saved rules are not touched.
pub async fn set_auto_reply_enabled(
    store: &ConfigStore,
    manager: &ConnectionManager,
    enabled: bool,
) -> Result<RevisionedConfig, String> {
    let config = store.set_auto_reply_enabled(enabled).await?;
    Ok(apply_rules(manager, config).await)
}
