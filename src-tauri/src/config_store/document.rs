//! The config file as a JSON document: parsing (preserving unknown root keys) and the atomic
//! write. Plain functions on a path: no locking, see `ConfigStore`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

use super::defaults::{builtin_templates, default_auto_reply_for};
use super::model::Template;
use super::validation::validate_templates;
use crate::auto_reply::{AutoReplyConfig, RawAutoReplyConfig};

pub const CONFIG_FILE_NAME: &str = "config.json";
pub(super) const TEMPLATES_KEY: &str = "templates";
pub(super) const AUTO_REPLY_KEY: &str = "auto_reply";

pub(super) static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// A parsed config file: the raw root (to preserve unknown keys) and its effective values.
pub(super) struct ConfigDocument {
    pub(super) root: Map<String, Value>,
    pub(super) templates: Vec<Template>,
    pub(super) auto_reply: AutoReplyConfig,
}

/// Reads the root object; a missing file is an empty root.
fn read_root(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(err) => return Err(format!("Cannot read {}: {err}", path.display())),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(format!(
            "{} must contain a JSON object at its root",
            path.display()
        )),
    }
}

/// Context-free errors: `read_config` adds the file name.
fn parse_templates(value: &Value) -> Result<Vec<Template>, String> {
    let templates: Vec<Template> =
        serde_json::from_value(value.clone()).map_err(|err| err.to_string())?;
    validate_templates(&templates)?;
    Ok(templates)
}

/// Context-free errors: `read_config` adds the file name.
fn parse_auto_reply(value: &Value) -> Result<AutoReplyConfig, String> {
    let raw: RawAutoReplyConfig =
        serde_json::from_value(value.clone()).map_err(|err| err.to_string())?;
    raw.parse()
}

/// The one parse path. Missing file or keys give defaults; corrupt values are errors.
/// Template references are not checked here (see `compile_with`).
pub(super) fn read_config(path: &Path) -> Result<ConfigDocument, String> {
    let root = read_root(path)?;
    let templates = match root.get(TEMPLATES_KEY) {
        None => builtin_templates(),
        Some(value) => parse_templates(value)
            .map_err(|err| format!("Invalid \"{TEMPLATES_KEY}\" in {}: {err}", path.display()))?,
    };
    let auto_reply = match root.get(AUTO_REPLY_KEY) {
        None => default_auto_reply_for(&templates),
        Some(value) => parse_auto_reply(value)
            .map_err(|err| format!("Invalid \"{AUTO_REPLY_KEY}\" in {}: {err}", path.display()))?,
    };
    Ok(ConfigDocument {
        root,
        templates,
        auto_reply,
    })
}

pub(super) fn write_root(path: &Path, root: Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|err| format!("Cannot serialize config: {err}"))?;
    write_atomic(path, text.as_bytes())
}

/// Writes next to the target then replaces it (rename replaces on Windows too).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir)
        .map_err(|err| format!("Cannot create config directory {}: {err}", dir.display()))?;

    let tmp = dir.join(format!(
        ".{CONFIG_FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();

    if let Err(err) = result {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Cannot write {}: {err}", path.display()));
    }
    Ok(())
}
