#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod auto_reply;
mod config_store;
mod emitter;
mod frames;
mod logger;
mod message_builder;
mod message_queue;
mod models;
mod session_commands;
mod translate;
mod transport;

use app_state::AppState;
use auto_reply::AutoReplyConfig;
use config_store::{ConfigLock, LoadedAutoReply, LoadedTemplates, Template};
use logger::AppLogger;
use message_builder::auto_build;
use models::{
    AutoBuildRequest, BuildResponse, ConnectRequest, FrontendLogEntry, LogLevel, SendRequest,
};
use tauri::{AppHandle, Manager, RunEvent, State};
use tokio::sync::Mutex;

use crate::emitter::Emitter;

/// Starts connection attempt `attempt` of the session `session_id` (created on first use).
/// The frontend chooses both, registers them before invoking, and ignores events of older attempts.
#[tauri::command]
async fn connect_socket(
    state: State<'_, Mutex<AppState>>,
    session_id: String,
    attempt: u64,
    req: ConnectRequest,
) -> Result<(), String> {
    log_request(&state, "connect", &session_id).await;
    session_commands::connect(&state, &session_id, attempt, req).await
}

/// Stops one session, which stays registered and can be reconnected.
#[tauri::command]
async fn disconnect_socket(
    state: State<'_, Mutex<AppState>>,
    session_id: String,
) -> Result<(), String> {
    log_request(&state, "disconnect", &session_id).await;
    session_commands::stop(&state, &session_id, false).await
}

/// Stops one session and forgets it.
#[tauri::command]
async fn close_session(
    state: State<'_, Mutex<AppState>>,
    session_id: String,
) -> Result<(), String> {
    log_request(&state, "close", &session_id).await;
    session_commands::stop(&state, &session_id, true).await
}

#[tauri::command]
async fn send_message(
    state: State<'_, Mutex<AppState>>,
    session_id: String,
    attempt: u64,
    payload: SendRequest,
) -> Result<(), String> {
    log_request(&state, "send_message", &session_id).await;
    session_commands::send(&state, &session_id, attempt, &payload).await
}

async fn log_request(state: &Mutex<AppState>, what: &str, session_id: &str) {
    state.lock().await.emitter.only_log(
        LogLevel::Inf,
        file!(),
        line!(),
        format!("{what} requested session={session_id}"),
    );
}

#[tauri::command]
async fn auto_build_message_cmd(
    state: State<'_, Mutex<AppState>>,
    req: AutoBuildRequest,
) -> Result<BuildResponse, String> {
    let state_val = state.lock().await;
    let logger = state_val.emitter.clone();

    logger.only_log(
        LogLevel::Inf,
        file!(),
        line!(),
        format!(
            "auto_build_message requested chars={}",
            req.input.chars().count()
        ),
    );
    auto_build(req).map_err(|err| {
        logger.only_log(
            LogLevel::Err,
            file!(),
            line!(),
            format!("auto_build_message failed: {err}"),
        );
        err.to_string()
    })
}

#[tauri::command]
async fn log_frontend(
    state: State<'_, Mutex<AppState>>,
    entry: FrontendLogEntry,
) -> Result<(), String> {
    let state_val = state.lock().await;
    state_val.emitter.log_frontend(&entry);
    Ok(())
}

#[tauri::command]
async fn load_templates(
    app: AppHandle,
    lock: State<'_, ConfigLock>,
) -> Result<LoadedTemplates, String> {
    let path = resolve_config_path(&app)?;
    session_commands::load_templates(&lock, &path).await
}

/// Saves the template library; refused when it would break an auto reply rule. The running
/// rules of every session are refreshed with the new templates.
#[tauri::command]
async fn save_templates(
    app: AppHandle,
    lock: State<'_, ConfigLock>,
    state: State<'_, Mutex<AppState>>,
    templates: Vec<Template>,
) -> Result<(), String> {
    let path = resolve_config_path(&app)?;
    session_commands::save_templates(&lock, &path, &state, &templates).await
}

/// Auto reply rules are global (every current and future session) and persisted in config.json.
#[tauri::command]
async fn load_auto_reply(
    app: AppHandle,
    lock: State<'_, ConfigLock>,
) -> Result<LoadedAutoReply, String> {
    let path = resolve_config_path(&app)?;
    session_commands::load_auto_reply(&lock, &path).await
}

#[tauri::command]
async fn save_auto_reply(
    app: AppHandle,
    lock: State<'_, ConfigLock>,
    state: State<'_, Mutex<AppState>>,
    config: AutoReplyConfig,
) -> Result<(), String> {
    let path = resolve_config_path(&app)?;
    session_commands::save_auto_reply(&lock, &path, &state, &config).await
}

/// Persists only the master switch (never the rules being edited).
#[tauri::command]
async fn set_auto_reply_enabled(
    app: AppHandle,
    lock: State<'_, ConfigLock>,
    state: State<'_, Mutex<AppState>>,
    enabled: bool,
) -> Result<(), String> {
    let path = resolve_config_path(&app)?;
    session_commands::set_auto_reply_enabled(&lock, &path, &state, enabled).await
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
            let logger = AppLogger::new(&handle).map_err(|err| err.to_string())?;
            logger.log_backend(
                LogLevel::Inf,
                file!(),
                line!(),
                "backend logger initialized",
            );
            let emitter = Emitter::new(handle, logger);
            app.manage(Mutex::new(AppState::new(emitter)));
            app.manage(ConfigLock(Mutex::new(())));
            // The persisted rules and templates are loaded before any command can connect.
            let path = resolve_config_path(app.handle())?;
            let state = app.state::<Mutex<AppState>>();
            let lock = app.state::<ConfigLock>();
            tauri::async_runtime::block_on(session_commands::init_auto_reply(&lock, &path, &state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_socket,
            disconnect_socket,
            close_session,
            send_message,
            auto_build_message_cmd,
            log_frontend,
            load_templates,
            save_templates,
            load_auto_reply,
            save_auto_reply,
            set_auto_reply_enabled,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // Best effort and never blocking: a busy lock is released by AppState's Drop.
                if let Some(state) = app.try_state::<Mutex<AppState>>() {
                    if let Ok(mut state) = state.try_lock() {
                        state.connection_manager.shutdown_now();
                    }
                }
            }
        });
}
