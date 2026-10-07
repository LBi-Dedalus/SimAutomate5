use crate::emitter::Emitter;
use crate::transport::ConnectionManager;

pub struct AppState {
    pub connection_manager: ConnectionManager,
    pub emitter: Emitter,
}

impl AppState {
    pub fn new(emitter: Emitter) -> Self {
        Self {
            connection_manager: ConnectionManager::new(emitter.clone()),
            emitter,
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.connection_manager.shutdown_now();
    }
}
