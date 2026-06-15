use tauri::AppHandle;

use crate::command_manager::CommandQueue;
use crate::logger::AppLogger;
use crate::models::AutoResponseConfig;

pub struct AppState {
    pub command_queue: CommandQueue,
    pub logger: AppLogger,
    pub desired_auto_response: AutoResponseConfig,
}

impl AppState {
    pub fn new(app: &AppHandle, logger: AppLogger) -> Self {
        Self {
            command_queue: CommandQueue::start(app, logger.clone()),
            logger,
            desired_auto_response: AutoResponseConfig::default(),
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.command_queue.close();
    }
}
