use crate::logger::AppLogger;
use crate::models::AutoResponseConfig;
use crate::transport::ConnectionQueue;


pub struct AppState {
    pub connection: Option<ConnectionQueue>,
    pub logger: AppLogger,
    pub desired_auto_response: AutoResponseConfig,
}

impl AppState {
    pub fn new(logger: AppLogger) -> Self {
        Self {
            connection: None,
            logger,
            desired_auto_response: AutoResponseConfig::default(),
        }
    }
}
