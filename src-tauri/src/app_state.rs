use crate::logger::AppLogger;
use crate::transport::ConnectionQueue;


pub struct AppState {
    pub connection: Option<ConnectionQueue>,
    pub logger: AppLogger,
}

impl AppState {
    pub fn new(logger: AppLogger) -> Self {
        Self {
            connection: None,
            logger,
        }
    }
}
