use crate::emitter::Emitter;
use crate::models::AutoResponseConfig;
use crate::transport::ConnectionManager;

pub struct AppState {
    pub connection_manager: ConnectionManager,
    pub emitter: Emitter,
    pub desired_auto_response: AutoResponseConfig,
}

impl AppState {
    pub fn new(emitter: Emitter) -> Self {
        Self {
            connection_manager: ConnectionManager::new(emitter.clone()),
            emitter,
            desired_auto_response: AutoResponseConfig::default(),
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.connection_manager.disconnect();
    }
}
