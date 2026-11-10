use std::sync::{Arc, Mutex};

use crate::ids::ConnectionId;
use crate::models::{MessagePayload, StatusPayload};
use crate::reporting::{ConnectionReporter, ConnectionReporterFactory, EventSink, NullLogger};

#[derive(Default)]
pub(super) struct Sink {
    pub(super) statuses: Mutex<Vec<StatusPayload>>,
    pub(super) messages: Mutex<Vec<MessagePayload>>,
}

impl EventSink for Sink {
    fn status(&self, payload: StatusPayload) -> Result<(), String> {
        self.statuses.lock().unwrap().push(payload);
        Ok(())
    }

    fn message(&self, payload: MessagePayload) -> Result<(), String> {
        self.messages.lock().unwrap().push(payload);
        Ok(())
    }

    fn notify(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
}

pub(super) fn reporter() -> (Arc<Sink>, ConnectionReporter) {
    let sink = Arc::new(Sink::default());
    let reporter = ConnectionReporterFactory::new(sink.clone(), Arc::new(NullLogger))
        .create(&ConnectionId::parse("s", 1).unwrap());
    (sink, reporter)
}
