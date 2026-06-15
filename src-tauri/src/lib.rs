#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod auto_response;
mod command_manager;
mod logger;
mod message_builder;
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
use std::process::Command;
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;

#[tauri::command]
async fn connect_socket(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    req: ConnectRequest,
) -> Result<(), String> {
    let state_val = state.lock().await;
    let logger = state_val.logger.clone();

    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        format!("connect requested ip={} port={}", req.ip, req.port),
    );

    let result = state_val
        .command_queue
        .send(crate::models::Command::Connect(req))
        .await;

    if let Err(err) = &result {
        logger.log_backend(
            LogLevel::Wrn,
            file!(),
            line!(),
            format!("connect queue send failed: {err}"),
        );
    }
    result
}

#[tauri::command]
async fn disconnect_socket(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let state_val = state.lock().await;
    let logger = state_val.logger.clone();
    logger.log_backend(LogLevel::Inf, file!(), line!(), "disconnect requested");

    let result = state_val
        .command_queue
        .send(crate::models::Command::Disconnect)
        .await;

    if let Err(err) = &result {
        logger.log_backend(
            LogLevel::Wrn,
            file!(),
            line!(),
            format!("disconnect queue send failed: {err}"),
        );
    }

    result
}

#[tauri::command]
async fn send_message(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    payload: SendRequest,
) -> Result<(), String> {
    let state_val = state.lock().await;
    let logger = state_val.logger.clone();

    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        format!(
            "send_message requested length={}",
            payload.message.chars().count()
        ),
    );

    let result = state_val
        .command_queue
        .send(crate::models::Command::SendMessage(payload))
        .await;

    if let Err(err) = &result {
        logger.log_backend(
            LogLevel::Wrn,
            file!(),
            line!(),
            format!("send_message queue send failed: {err}"),
        );
    }

    result
}

#[tauri::command]
async fn update_auto_response(
    _app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    config: AutoResponseConfig,
) -> Result<(), String> {
    let mut state_val = state.lock().await;
    let logger = state_val.logger.clone();

    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        format!(
            "update_auto_response requested enabled={} protocol={:?}",
            config.enabled, config.protocol
        ),
    );

    state_val.desired_auto_response = config.clone();

    // let result = state_val
    //     .command_queue
    //     .send(crate::models::Command::SendMessage(payload))
    //     .await;

    // if let Err(err) = &result {
    //     logger.log_backend(
    //         LogLevel::Wrn,
    //         file!(),
    //         line!(),
    //         format!("update_auto_response queue send failed: {err}"),
    //     );
    // }

    // result
    Ok(())
}

#[tauri::command]
async fn auto_build_message_cmd(
    state: State<'_, Mutex<AppState>>,
    req: AutoBuildRequest,
) -> Result<BuildResponse, String> {
    let state_val = state.lock().await;
    let logger = state_val.logger.clone();

    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        format!(
            "auto_build_message requested input_length={}",
            req.input.chars().count()
        ),
    );
    auto_build(req).map_err(|err| {
        logger.log_backend(
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
    state_val.logger.log_frontend(&entry);
    Ok(())
}

#[tauri::command]
async fn open_logs_folder(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let state_val = state.lock().await;
    let logger = state_val.logger.clone();
    let log_dir = logger.log_directory();

    logger.log_backend(
        LogLevel::Inf,
        file!(),
        line!(),
        format!("open_logs_folder requested path={}", log_dir.display()),
    );

    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("explorer");
        command.arg(&log_dir);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(&log_dir);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(&log_dir);
        command
    };

    command.spawn().map_err(|err| {
        logger.log_backend(
            LogLevel::Err,
            file!(),
            line!(),
            format!("failed to open logs folder {}: {err}", log_dir.display()),
        );
        err.to_string()
    })?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let logger = AppLogger::new(&app.handle()).map_err(|err| err.to_string())?;
            logger.log_backend(
                LogLevel::Inf,
                file!(),
                line!(),
                "backend logger initialized",
            );
            app.manage(Mutex::new(AppState::new(&app.handle(), logger)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_socket,
            disconnect_socket,
            send_message,
            auto_build_message_cmd,
            update_auto_response,
            log_frontend,
            open_logs_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
