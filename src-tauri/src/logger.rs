use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use std::ffi::OsString;
use std::fs::{create_dir_all, metadata, remove_file, rename, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

use crate::models::{FrontendLogEntry, LogLevel};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const MAX_LOG_FILES: u8 = 5;

#[derive(Clone)]
pub struct AppLogger {
    state: Arc<LoggerState>,
}

struct LoggerState {
    write_lock: Mutex<()>,
    backend_path: PathBuf,
    frontend_path: PathBuf,
}

enum LogTarget {
    Backend,
    Frontend,
}

impl AppLogger {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let log_dir = app
            .path()
            .app_log_dir()
            .or_else(|_| app.path().app_data_dir())
            .context("failed to resolve app log directory")?;

        create_dir_all(&log_dir).context("failed to create log directory")?;

        let backend_path = log_dir.join("backend.log");
        let frontend_path = log_dir.join("frontend.log");

        Ok(Self {
            state: Arc::new(LoggerState {
                write_lock: Mutex::new(()),
                backend_path,
                frontend_path,
            }),
        })
    }

    pub fn log_backend(&self, level: LogLevel, file: &str, line: u32, message: impl AsRef<str>) {
        let location = format!("{file}:{line}");
        self.write(LogTarget::Backend, level, &location, message.as_ref());
    }

    pub fn log_frontend(&self, entry: &FrontendLogEntry) {
        let location = if entry.location.trim().is_empty() {
            "unknown:0"
        } else {
            entry.location.trim()
        };
        self.write(
            LogTarget::Frontend,
            entry.level.clone(),
            location,
            &entry.message,
        );
    }

    fn write(&self, target: LogTarget, level: LogLevel, location: &str, message: &str) {
        if let Err(err) = self.write_inner(target, level, location, message) {
            eprintln!("logger failure: {err}");
        }
    }

    fn write_inner(
        &self,
        target: LogTarget,
        level: LogLevel,
        location: &str,
        message: &str,
    ) -> Result<()> {
        let _guard = self
            .state
            .write_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to acquire logger mutex"))?;

        let file_path = match target {
            LogTarget::Backend => &self.state.backend_path,
            LogTarget::Frontend => &self.state.frontend_path,
        };

        rotate_if_needed(file_path)?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_path)
            .with_context(|| format!("failed to open log file {}", file_path.display()))?;

        let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let cleaned_message = message.replace('\n', "\\n");

        writeln!(
            file,
            "{timestamp} | {} | {} | {cleaned_message}",
            location,
            level.as_str(),
        )
        .with_context(|| format!("failed writing log line to {}", file_path.display()))?;

        Ok(())
    }
}

fn rotate_if_needed(path: &Path) -> Result<()> {
    let size = metadata(path).map(|m| m.len()).unwrap_or(0);
    if size < MAX_LOG_BYTES {
        return Ok(());
    }

    let oldest = rotated_path(path, MAX_LOG_FILES);
    if oldest.exists() {
        let _ = remove_file(&oldest);
    }

    for index in (1..MAX_LOG_FILES).rev() {
        let src = rotated_path(path, index);
        if src.exists() {
            let dst = rotated_path(path, index + 1);
            let _ = rename(src, dst);
        }
    }

    if path.exists() {
        let _ = rename(path, rotated_path(path, 1));
    }

    Ok(())
}

fn rotated_path(path: &Path, index: u8) -> PathBuf {
    let mut os = OsString::from(path.as_os_str());
    os.push(format!(".{index}"));
    PathBuf::from(os)
}