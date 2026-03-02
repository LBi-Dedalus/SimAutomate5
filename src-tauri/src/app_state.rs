use crate::transport::ConnectionQueue;


pub struct AppState {
    pub connection: Option<ConnectionQueue>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: None,
        }
    }
}
