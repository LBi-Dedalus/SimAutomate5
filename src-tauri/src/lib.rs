#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod auto_response;
mod emitter;
mod logger;
mod message_builder;
mod message_queue;
mod models;
mod translate;
mod transport;

use app_state::AppState;
use logger::AppLogger;
use message_builder::auto_build;
use models::{
    AutoBuildRequest, AutoResponseConfig, BuildResponse, ConnectRequest, FrontendLogEntry,
    LogLevel, SendRequest,
};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;

use crate::emitter::Emitter;

#[tauri::command]
async fn connect_socket(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    req: ConnectRequest,
) -> Result<(), String> {
    let mut state_val = state.lock().await;
    let logger = state_val.emitter.clone();

    logger.only_log(
        LogLevel::Inf,
        file!(),
        line!(),
        format!("connect requested ip={} port={}", req.ip, req.port),
    );

    state_val.connection_manager.connect(req).await;

    Ok(())
}

#[tauri::command]
async fn disconnect_socket(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let mut state_val = state.lock().await;
    let logger = state_val.emitter.clone();
    logger.only_log(LogLevel::Inf, file!(), line!(), "disconnect requested");

    state_val.connection_manager.disconnect().await;

    Ok(())
}

#[tauri::command]
async fn send_message(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    payload: SendRequest,
) -> Result<(), String> {
    let mut state_val = state.lock().await;
    let logger = state_val.emitter.clone();

    logger.only_log(
        LogLevel::Inf,
        file!(),
        line!(),
        format!(
            "send_message requested length={}",
            payload.message.chars().count()
        ),
    );

    state_val
        .connection_manager
        .send_user_message(&payload)
        .await;

    Ok(())
}

#[tauri::command]
async fn update_auto_response(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    config: AutoResponseConfig,
) -> Result<(), String> {
    let mut state_val = state.lock().await;
    let logger = state_val.emitter.clone();

    logger.only_log(
        LogLevel::Inf,
        file!(),
        line!(),
        format!(
            "update_auto_response requested enabled={} protocol={:?}",
            config.enabled, config.protocol
        ),
    );

    state_val.desired_auto_response = config.clone();
    state_val
        .connection_manager
        .update_auto_response(config)
        .await;

    Ok(())
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
            "auto_build_message requested input_length={}",
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_socket,
            disconnect_socket,
            send_message,
            auto_build_message_cmd,
            update_auto_response,
            log_frontend,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
