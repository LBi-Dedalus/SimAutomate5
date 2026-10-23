//! Where the backend's logs and events go: the log files and the frontend events of each connection.

mod connection_reporter;
mod logger;

pub use connection_reporter::{ConnectionReporter, ConnectionReporterFactory, TauriSink};
pub use logger::{AppLogger, Logger};

#[cfg(test)]
pub use connection_reporter::EventSink;
#[cfg(test)]
pub use logger::NullLogger;
