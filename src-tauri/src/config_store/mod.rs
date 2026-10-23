//! Persistence of message templates and auto reply rules in `<app config dir>/config.json`.
//!
//! The file is a JSON object. Only the `templates` and `auto_reply` keys are owned by the app;
//! every other root field is preserved untouched when saving. The file is never overwritten
//! when it exists but cannot be understood (invalid JSON, root that is not an object, or a
//! malformed `templates` / `auto_reply` value): the error is reported instead.
//!
//! The parsed file is the single source of truth (`RevisionedConfig`), read by one path
//! (`document::read_config`). Missing keys get defaults: a missing `templates` is the built-in
//! library and a missing `auto_reply` is the default rules (enabled). Nothing is written for a
//! missing key until the user saves that part: each save writes only the key it owns.
//!
//! Rules reference templates by id, so both keys are cross-validated on every write: a template
//! save is refused when it would break a rule (default rules included), and a rule save is
//! refused when it references a missing or unusable template. Every `RevisionedConfig` carries
//! the compiled rule set; the caller applies it to the running connections (the
//! `ConnectionManager` ignores a configuration older than the one it already runs).
//!
//! `ConfigStore` owns the file path and the current `RevisionedConfig`, behind the lock that
//! serializes the whole read -> validate -> write sequence. The plain file functions take a path
//! and do no locking: they are private building blocks, so every access goes through the store.
//!
//! Layout: `model` (types), `document` (JSON parsing and atomic write), `validation`,
//! `defaults` (built-ins).

mod defaults;
mod document;
mod model;
mod validation;

use std::path::{Path, PathBuf};

use crate::auto_reply::{AutoReplyConfig, AutoReplyRuleSet};

#[cfg(test)]
pub(crate) use defaults::builtin_templates;
pub(crate) use document::config_path;
// `AutobuildMode` is part of `Template`; only tests name it outside this module.
#[allow(unused_imports)]
pub use model::AutobuildMode;
pub use model::{RevisionedConfig, Template, TemplateId, TemplateVariable};

use defaults::default_auto_reply_for;
use document::{read_config, write_root, ConfigDocument, AUTO_REPLY_KEY, TEMPLATES_KEY};
use validation::{compile_at, first_rule_missing_template, validate_templates};

/// The config file and the current configuration, behind the lock serializing access to it
/// (managed separately from the `ConnectionManager`).
pub struct ConfigStore {
    path: PathBuf,
    /// `None` until the first successful load or save. Loads are served from it; saves always
    /// read the file (so a manual edit is taken into account) and replace it.
    current: tokio::sync::Mutex<Option<RevisionedConfig>>,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            current: tokio::sync::Mutex::new(None),
        }
    }

    /// The current configuration. The first call reads and validates the file (revision 0);
    /// loads never bump the revision.
    pub async fn load(&self) -> Result<RevisionedConfig, String> {
        let mut current = self.current.lock().await;
        if let Some(config) = current.as_ref() {
            return Ok(config.clone());
        }
        let config = load_from(&self.path, 0)?;
        *current = Some(config.clone());
        Ok(config)
    }

    /// Saves the templates (refused when a rule would break).
    pub async fn save_templates(&self, templates: &[Template]) -> Result<RevisionedConfig, String> {
        self.save_with(|path, revision| save_templates_to(path, revision, templates))
            .await
    }

    pub async fn save_auto_reply(
        &self,
        config: &AutoReplyConfig,
    ) -> Result<RevisionedConfig, String> {
        self.save_with(|path, revision| save_auto_reply_to(path, revision, config))
            .await
    }

    /// Persists the master switch only; the saved rules are not touched.
    pub async fn set_auto_reply_enabled(&self, enabled: bool) -> Result<RevisionedConfig, String> {
        self.save_with(|path, revision| set_auto_reply_enabled_in(path, revision, enabled))
            .await
    }

    /// Runs `op` under the lock with the next revision. Only on success is the revision
    /// consumed and the new configuration stored.
    async fn save_with(
        &self,
        op: impl FnOnce(&Path, u64) -> Result<RevisionedConfig, String>,
    ) -> Result<RevisionedConfig, String> {
        let mut current = self.current.lock().await;
        let revision = current.as_ref().map_or(0, |config| config.revision) + 1;
        let next = op(&self.path, revision)?;
        *current = Some(next.clone());
        Ok(next)
    }
}

/// Reads and fully validates the configuration (shape and template references).
fn load_from(path: &Path, revision: u64) -> Result<RevisionedConfig, String> {
    let doc = read_config(path)?;
    let rules = compile_at(path, &doc.auto_reply, &doc.templates)?;
    Ok(RevisionedConfig {
        revision,
        templates: doc.templates,
        auto_reply: doc.auto_reply,
        rules,
    })
}

/// Replaces the saved rules (the `auto_reply` key is the only one written).
fn save_auto_reply_to(
    path: &Path,
    revision: u64,
    config: &AutoReplyConfig,
) -> Result<RevisionedConfig, String> {
    let mut doc = read_config(path)?;
    doc.auto_reply = config.clone();
    write_auto_reply(path, revision, doc)
}

/// Changes only the persisted master switch; the rules are written back untouched.
fn set_auto_reply_enabled_in(
    path: &Path,
    revision: u64,
    enabled: bool,
) -> Result<RevisionedConfig, String> {
    let mut doc = read_config(path)?;
    doc.auto_reply.set_enabled(enabled);
    write_auto_reply(path, revision, doc)
}

fn write_auto_reply(
    path: &Path,
    revision: u64,
    mut doc: ConfigDocument,
) -> Result<RevisionedConfig, String> {
    let rules = compile_at(path, &doc.auto_reply, &doc.templates)?;
    doc.root.insert(
        AUTO_REPLY_KEY.to_string(),
        serde_json::to_value(&doc.auto_reply).map_err(|err| err.to_string())?,
    );
    write_root(path, doc.root)?;
    Ok(RevisionedConfig {
        revision,
        templates: doc.templates,
        auto_reply: doc.auto_reply,
        rules,
    })
}

/// Replaces the template library (the `templates` key is the only one written). The `auto_reply`
/// key is never written: while it is missing the default rules stay in effect.
fn save_templates_to(
    path: &Path,
    revision: u64,
    templates: &[Template],
) -> Result<RevisionedConfig, String> {
    validate_templates(templates)?;

    let mut doc = read_config(path)?;
    let key_missing = !doc.root.contains_key(AUTO_REPLY_KEY);
    // Rules (saved or default) that reference a template must keep working with the new library.
    if let Some((template_id, rule_name)) = first_rule_missing_template(&doc.auto_reply, templates)
    {
        return Err(format!(
            "Refused: the template \"{template_id}\" is used by the auto reply rule \"{rule_name}\""
        ));
    }
    AutoReplyRuleSet::compile(&doc.auto_reply, templates)
        .map_err(|err| format!("Refused: the auto reply rules would no longer work: {err}"))?;
    doc.templates = templates.to_vec();
    if key_missing {
        // Same as a reload would give for the new library.
        doc.auto_reply = default_auto_reply_for(templates);
    }
    let rules = compile_at(path, &doc.auto_reply, templates)?;
    doc.root.insert(
        TEMPLATES_KEY.to_string(),
        serde_json::to_value(templates).map_err(|err| err.to_string())?,
    );
    write_root(path, doc.root)?;
    Ok(RevisionedConfig {
        revision,
        templates: doc.templates,
        auto_reply: doc.auto_reply,
        rules,
    })
}

#[cfg(test)]
mod tests;
