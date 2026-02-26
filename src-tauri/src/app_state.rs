use std::sync::Arc;

use tauri::async_runtime::{JoinHandle, Mutex};
use tokio::net::tcp::OwnedWriteHalf;

use crate::models::{AutoResponseConfig, Protocol};

pub struct ActiveConnection {
    pub writer: Arc<Mutex<OwnedWriteHalf>>,
    pub reader_task: JoinHandle<()>,
    pub protocol: Protocol,
    pub _remote: String,
}

pub struct AppState {
    pub connection: Mutex<Option<ActiveConnection>>,
    pub auto_response: Arc<Mutex<AutoResponseConfig>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: Mutex::new(None),
            auto_response: Arc::new(Mutex::new(AutoResponseConfig::default())),
        }
    }
}
