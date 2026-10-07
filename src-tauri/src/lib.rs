#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod auto_response;
mod config_store;
mod emitter;
mod logger;
mod message_builder;
mod message_queue;
mod models;
mod session_commands;
mod translate;
mod transport;

use app_state::AppState;
use config_store::{ConfigLock, LoadedTemplates, Template};
use logger::AppLogger;
use message_builder::auto_build;
use models::{
    AutoBuildRequest, AutoResponseConfig, BuildResponse, ConnectRequest, FrontendLogEntry,
    LogLevel, SendRequest,
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

/// The auto-response configuration is global: it applies to every current and future session.
#[tauri::command]
async fn update_auto_response(
    state: State<'_, Mutex<AppState>>,
    config: AutoResponseConfig,
) -> Result<(), String> {
    state.lock().await.emitter.only_log(
        LogLevel::Inf,
        file!(),
        line!(),
        format!("update_auto_response requested enabled={}", config.enabled),
    );
    session_commands::update_auto_response(&state, config).await;
    Ok(())
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
    let _guard = lock.0.lock().await;
    let path = resolve_config_path(&app)?;
    config_store::load_templates_from(&path)
}

#[tauri::command]
async fn save_templates(
    app: AppHandle,
    lock: State<'_, ConfigLock>,
    templates: Vec<Template>,
) -> Result<(), String> {
    let _guard = lock.0.lock().await;
    let path = resolve_config_path(&app)?;
    config_store::save_templates_to(&path, &templates)
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_socket,
            disconnect_socket,
            close_session,
            send_message,
            auto_build_message_cmd,
            update_auto_response,
            log_frontend,
            load_templates,
            save_templates,
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
