#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod auto_response;
mod message_builder;
mod models;
mod transport;
mod translate;

use app_state::AppState;
use message_builder::{auto_build, build as build_message};
use models::{AutoBuildRequest, AutoResponseConfig, BuildRequest, BuildResponse, ConnectRequest, SendRequest};
use tauri::{AppHandle, State};
use transport::{connect_and_spawn, disconnect_active, send_user_message};

#[tauri::command]
async fn connect_socket(app: AppHandle, state: State<'_, AppState>, req: ConnectRequest) -> Result<(), String> {
    connect_and_spawn(app, state, req).await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn disconnect_socket(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    disconnect_active(&app, &state).await;
    Ok(())
}

#[tauri::command]
async fn send_message(app: AppHandle, state: State<'_, AppState>, payload: SendRequest) -> Result<(), String> {
    send_user_message(app, state, payload)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn update_auto_response(state: State<'_, AppState>, config: AutoResponseConfig) -> Result<(), String> {
    let mut stored = state.auto_response.lock().await;
    *stored = config;
    Ok(())
}

#[tauri::command]
async fn build_message_cmd(req: BuildRequest) -> Result<BuildResponse, String> {
    build_message(req).map_err(|err| err.to_string())
}

#[tauri::command]
async fn auto_build_message_cmd(req: AutoBuildRequest) -> Result<BuildResponse, String> {
    auto_build(req).map_err(|err| err.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            connect_socket,
            disconnect_socket,
            send_message,
            build_message_cmd,
            auto_build_message_cmd,
            update_auto_response,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
