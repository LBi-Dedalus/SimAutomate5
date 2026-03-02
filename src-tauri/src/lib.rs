#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod auto_response;
mod message_builder;
mod models;
mod translate;
mod transport;

use app_state::AppState;
use message_builder::auto_build;
use models::{AutoBuildRequest, AutoResponseConfig, BuildResponse, ConnectRequest, SendRequest};
use tauri::{AppHandle, State};
use tokio::sync::Mutex;

use crate::transport::ConnectionMessage;

#[tauri::command]
async fn connect_socket(
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    req: ConnectRequest,
) -> Result<(), String> {
    let mut state_val = state.lock().await;
    
    if let Some(queue) = state_val.connection.as_ref() {
        let _ = queue.send(ConnectionMessage::Disconnect).await;
    }

    state_val.connection = transport::start(&app, req).await;
    Ok(())
}

#[tauri::command]
async fn disconnect_socket(
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    let state_val = state.lock().await;

    if let Some(queue) = state_val.connection.as_ref() {
        let _ = queue.send(ConnectionMessage::Disconnect).await;
    }

    Ok(())
}

#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    payload: SendRequest,
) -> Result<(), String> {
    let state_val = state.lock().await;

    if let Some(queue) = state_val.connection.as_ref() {
        queue
            .send(ConnectionMessage::SendMessage(payload))
            .await
            .map_err(|err| err.to_string())?;
    }

    Ok(())
}

#[tauri::command]
async fn update_auto_response(
    state: State<'_, Mutex<AppState>>,
    config: AutoResponseConfig,
) -> Result<(), String> {
    let state_val = state.lock().await;

    if let Some(queue) = state_val.connection.as_ref() {
        queue
            .send(ConnectionMessage::SetAutoResponse(config))
            .await
            .map_err(|err| err.to_string())?;
    }

    Ok(())
}

#[tauri::command]
async fn auto_build_message_cmd(req: AutoBuildRequest) -> Result<BuildResponse, String> {
    auto_build(req).map_err(|err| err.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            connect_socket,
            disconnect_socket,
            send_message,
            auto_build_message_cmd,
            update_auto_response,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
