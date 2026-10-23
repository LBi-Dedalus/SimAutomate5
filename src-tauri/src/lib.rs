#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auto_build;
mod auto_reply;
mod config_store;
mod ids;
mod message_orchestrator;
mod models;
mod reporting;
mod session_commands;
mod translate;
mod transport;

use auto_build::auto_build;
use auto_reply::AutoReplyConfig;
use config_store::{ConfigStore, RevisionedConfig, Template};
use models::{
    AutoBuildRequest, BuildResponse, ConnectRequest, FrontendLogEntry, LogLevel, SendRequest,
};
use reporting::{AppLogger, ConnectionReporterFactory, Logger, TauriSink};
use std::sync::Arc;
use tauri::{AppHandle, Manager, RunEvent, State};
use transport::ConnectionManager;

type SharedLogger = Arc<dyn Logger>;

/// Starts connection attempt `attempt` of the session `session_id` (created on first use).
/// The frontend chooses both, registers them before invoking, and ignores events of older attempts.
#[tauri::command]
async fn connect_socket(
    manager: State<'_, ConnectionManager>,
    logger: State<'_, SharedLogger>,
    session_id: String,
    attempt: u64,
    req: ConnectRequest,
) -> Result<(), String> {
    log_request(&**logger, "connect", &session_id);
    session_commands::connect(&manager, &session_id, attempt, req).await
}

/// Stops one session, which stays registered and can be reconnected.
#[tauri::command]
async fn disconnect_socket(
    manager: State<'_, ConnectionManager>,
    logger: State<'_, SharedLogger>,
    session_id: String,
) -> Result<(), String> {
    log_request(&**logger, "disconnect", &session_id);
    session_commands::stop(
        &manager,
        &session_id,
        session_commands::StopMode::Disconnect,
    )
    .await
}

/// Stops one session and forgets it.
#[tauri::command]
async fn close_session(
    manager: State<'_, ConnectionManager>,
    logger: State<'_, SharedLogger>,
    session_id: String,
) -> Result<(), String> {
    log_request(&**logger, "close", &session_id);
    session_commands::stop(&manager, &session_id, session_commands::StopMode::Close).await
}

#[tauri::command]
async fn send_message(
    manager: State<'_, ConnectionManager>,
    logger: State<'_, SharedLogger>,
    session_id: String,
    attempt: u64,
    payload: SendRequest,
) -> Result<(), String> {
    log_request(&**logger, "send_message", &session_id);
    session_commands::send(&manager, &session_id, attempt, &payload).await
}

fn log_request(logger: &dyn Logger, what: &str, session_id: &str) {
    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        &format!("{what} requested session={session_id}"),
    );
}

#[tauri::command]
async fn auto_build_message_cmd(
    logger: State<'_, SharedLogger>,
    req: AutoBuildRequest,
) -> Result<BuildResponse, String> {
    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        &format!(
            "auto_build_message requested chars={}",
            req.input.chars().count()
        ),
    );
    auto_build(req).map_err(|err| {
        logger.log_backend(
            LogLevel::Err,
            file!(),
            line!(),
            &format!("auto_build_message failed: {err}"),
        );
        err.to_string()
    })
}

#[tauri::command]
async fn log_frontend(
    logger: State<'_, SharedLogger>,
    entry: FrontendLogEntry,
) -> Result<(), String> {
    logger.log_frontend(&entry);
    Ok(())
}

/// The effective templates and auto reply rules; auto reply rules are global (every current
/// and future session) and persisted in config.json.
#[tauri::command]
async fn load_config(store: State<'_, ConfigStore>) -> Result<RevisionedConfig, String> {
    session_commands::load_config(&store).await
}

/// Saves the template library; refused when it would break an auto reply rule. The running
/// rules of every session are refreshed with the new templates.
#[tauri::command]
async fn save_templates(
    store: State<'_, ConfigStore>,
    manager: State<'_, ConnectionManager>,
    templates: Vec<Template>,
) -> Result<RevisionedConfig, String> {
    session_commands::save_templates(&store, &manager, &templates).await
}

#[tauri::command]
async fn save_auto_reply(
    store: State<'_, ConfigStore>,
    manager: State<'_, ConnectionManager>,
    config: AutoReplyConfig,
) -> Result<RevisionedConfig, String> {
    session_commands::save_auto_reply(&store, &manager, &config).await
}

/// Persists only the master switch (never the rules being edited).
#[tauri::command]
async fn set_auto_reply_enabled(
    store: State<'_, ConfigStore>,
    manager: State<'_, ConnectionManager>,
    enabled: bool,
) -> Result<RevisionedConfig, String> {
    session_commands::set_auto_reply_enabled(&store, &manager, enabled).await
}

fn resolve_config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| format!("Cannot resolve the config directory: {err}"))?;
    Ok(config_store::config_path(&dir))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let logger: SharedLogger =
                Arc::new(AppLogger::new(&handle).map_err(|err| err.to_string())?);
            logger.log_backend(
                LogLevel::Inf,
                file!(),
                line!(),
                "backend logger initialized",
            );
            let factory =
                ConnectionReporterFactory::new(Arc::new(TauriSink::new(handle)), logger.clone());
            app.manage(ConnectionManager::new(factory));
            app.manage(logger);
            // Resolved once. As before, a failure aborts startup (setup returns the error).
            let path = resolve_config_path(app.handle())?;
            app.manage(ConfigStore::new(path));
            // The persisted rules and templates are loaded before any command can connect.
            let manager = app.state::<ConnectionManager>();
            let logger = app.state::<SharedLogger>();
            let store = app.state::<ConfigStore>();
            tauri::async_runtime::block_on(session_commands::init_auto_reply(
                &store, &manager, &**logger,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_socket,
            disconnect_socket,
            close_session,
            send_message,
            auto_build_message_cmd,
            log_frontend,
            load_config,
            save_templates,
            save_auto_reply,
            set_auto_reply_enabled,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // Best effort and never blocking: a busy lock is released by the manager's Drop.
                if let Some(manager) = app.try_state::<ConnectionManager>() {
                    manager.shutdown_now();
                }
            }
        });
}
