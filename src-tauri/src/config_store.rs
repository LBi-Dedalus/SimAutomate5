//! Persistence of message templates and auto reply rules in `<app config dir>/config.json`.
//!
//! The file is a JSON object. Only the `templates` and `auto_reply` keys are owned by the app;
//! every other root field is preserved untouched when saving. The file is never overwritten
//! when it exists but cannot be understood (invalid JSON, root that is not an object, or a
//! malformed `templates` / `auto_reply` value): the error is reported instead.
//!
//! The parsed file is the single source of truth (`EffectiveConfig`), read by one path
//! (`read_config`). Missing keys get defaults: a missing `templates` is the built-in library and
//! a missing `auto_reply` is the default rules (enabled). Nothing is written for a missing key
//! until the user saves that part: each save writes only the key it owns.
//!
//! Rules reference templates by id, so both keys are cross-validated on every write: a template
//! save is refused when it would break a rule (default rules included), and a rule save is
//! refused when it references a missing or unusable template. Every successful write returns
//! the effective configuration and the compiled rule set the caller must apply to the running
//! connections.
//!
//! `ConfigStore` owns the file path and the lock that serializes the whole read -> validate ->
//! write sequence (plus applying the result to the running connections, see `ConfigStore`).
//! The plain file functions below it take a path and do no locking: they are private building
//! blocks, so every access goes through the store.

use std::fs;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::auto_reply::{Action, AutoReplyConfig, Condition, Operator, Rule, RuleSet, Trigger};

pub const CONFIG_FILE_NAME: &str = "config.json";
const TEMPLATES_KEY: &str = "templates";
const AUTO_REPLY_KEY: &str = "auto_reply";

/// The config file and the lock serializing access to it (managed separately from `AppState`).
/// The lock guards the revision counter: it grows on every successful save.
pub struct ConfigStore {
    path: PathBuf,
    revision: tokio::sync::Mutex<u64>,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            revision: tokio::sync::Mutex::new(0),
        }
    }

    /// Reads and validates the configuration with its current revision (loads never bump it).
    /// The compiled rules are returned too, for the caller to apply (startup).
    pub async fn load(&self) -> Result<(u64, Applied), String> {
        let guard = self.revision.lock().await;
        load_from(&self.path).map(|applied| (*guard, applied))
    }

    /// Like `load`, with `apply` run on the compiled rules while the lock is still held.
    pub async fn load_and_apply<F, Fut>(&self, apply: F) -> Result<RevisionedConfig, String>
    where
        F: FnOnce(Arc<RuleSet>) -> Fut,
        Fut: Future<Output = ()>,
    {
        let guard = self.revision.lock().await;
        let applied = load_from(&self.path)?;
        apply(applied.rules).await;
        Ok(RevisionedConfig {
            revision: *guard,
            config: applied.config,
        })
    }

    /// Saves the templates (refused when a rule would break).
    pub async fn save_templates<F, Fut>(
        &self,
        templates: &[Template],
        apply: F,
    ) -> Result<RevisionedConfig, String>
    where
        F: FnOnce(Arc<RuleSet>) -> Fut,
        Fut: Future<Output = ()>,
    {
        self.save_with(|path| save_templates_to(path, templates), apply)
            .await
    }

    pub async fn save_auto_reply<F, Fut>(
        &self,
        config: &AutoReplyConfig,
        apply: F,
    ) -> Result<RevisionedConfig, String>
    where
        F: FnOnce(Arc<RuleSet>) -> Fut,
        Fut: Future<Output = ()>,
    {
        self.save_with(|path| save_auto_reply_to(path, config), apply)
            .await
    }

    /// Persists the master switch only; the saved rules are not touched.
    pub async fn set_auto_reply_enabled<F, Fut>(
        &self,
        enabled: bool,
        apply: F,
    ) -> Result<RevisionedConfig, String>
    where
        F: FnOnce(Arc<RuleSet>) -> Fut,
        Fut: Future<Output = ()>,
    {
        self.save_with(|path| set_auto_reply_enabled_in(path, enabled), apply)
            .await
    }

    /// Runs `op` under the lock. Only on success: bumps the revision, then runs `apply` with the
    /// validated rules before the lock is released, so applications happen in revision order.
    async fn save_with<F, Fut>(
        &self,
        op: impl FnOnce(&Path) -> Result<Applied, String>,
        apply: F,
    ) -> Result<RevisionedConfig, String>
    where
        F: FnOnce(Arc<RuleSet>) -> Fut,
        Fut: Future<Output = ()>,
    {
        let mut guard = self.revision.lock().await;
        let applied = op(&self.path)?;
        *guard += 1;
        let revision = *guard;
        apply(applied.rules).await;
        Ok(RevisionedConfig {
            revision,
            config: applied.config,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateVariable {
    pub name: String,
    #[serde(default)]
    pub default: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub payload: String,
    #[serde(default)]
    pub variables: Vec<TemplateVariable>,
    /// When true, the UI runs the resolved message through Autobuild (MLLP/ASTM framing)
    /// before sending it or loading it in the composer. Auto-reply rules ignore it.
    #[serde(default)]
    pub autobuild: bool,
    /// Autobuild "No ETB" option; only meaningful when `autobuild` is true.
    #[serde(default)]
    pub autobuild_no_etb: bool,
}

pub(crate) fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// Built-in templates. Each HL7 frame is ONE line: the backend sends every real
/// line break as a separate frame, so segments are separated with `<CR>` tokens.
pub(crate) fn builtin_templates() -> Vec<Template> {
    vec![
        Template {
            id: "builtin-hl7-qry-a19".to_string(),
            name: "HL7 QRY^A19 - Patient query".to_string(),
            description: "MLLP-framed patient query.".to_string(),
            payload: "<VT>MSH|^~\\&|SIMAUTOMATE|LAB|HIS|HOSP|{{NOW}}||QRY^A19|{{CONTROL_ID}}|P|2.4|||AL|NE|<CR>QRD|{{NOW}}|R|I|Q{{CONTROL_ID}}|||1^RD|{{PATIENT_ID}}|DEM|ALL|<CR><FS><CR>".to_string(),
            variables: vec![TemplateVariable {
                name: "PATIENT_ID".to_string(),
                default: String::new(),
            }],
            autobuild: false,
            autobuild_no_etb: false,
        },
        Template {
            id: "builtin-hl7-ack-o21".to_string(),
            name: "HL7 ACK^O21 - Acknowledgement".to_string(),
            description: "MLLP-framed acknowledgement of a received message.".to_string(),
            payload: "<VT>MSH|^~\\&|SIMAUTOMATE|LAB|HIS|HOSP|{{NOW}}||ACK^O21|{{CONTROL_ID}}|P|2.4<CR>MSA|AA|{{REQ_CONTROL_ID}}<CR><FS><CR>".to_string(),
            variables: vec![TemplateVariable {
                name: "REQ_CONTROL_ID".to_string(),
                default: String::new(),
            }],
            autobuild: false,
            autobuild_no_etb: false,
        },
        Template {
            id: "builtin-astm-enq".to_string(),
            name: "ASTM ENQ".to_string(),
            description: "Starts an ASTM transmission.".to_string(),
            payload: "<ENQ>".to_string(),
            variables: Vec::new(),
            autobuild: false,
            autobuild_no_etb: false,
        },
        Template {
            id: "builtin-astm-eot".to_string(),
            name: "ASTM EOT".to_string(),
            description: "Ends an ASTM transmission.".to_string(),
            payload: "<EOT>".to_string(),
            variables: Vec::new(),
            autobuild: false,
            autobuild_no_etb: false,
        },
    ]
}

/// Default auto reply configuration, used until the `auto_reply` key is first saved.
fn default_auto_reply() -> AutoReplyConfig {
    let rule = |id: &str, name: &str, trigger, condition, action| Rule {
        id: id.to_string(),
        name: name.to_string(),
        enabled: true,
        trigger,
        condition,
        action,
        delay_ms: 0,
    };
    let literal_ack = || Action::Literal {
        text: "<ACK>".to_string(),
    };
    AutoReplyConfig {
        enabled: true,
        rules: vec![
            rule(
                "default-astm-enq",
                "Reply to ASTM ENQ",
                Trigger::AstmEnq,
                None,
                literal_ack(),
            ),
            rule(
                "default-astm-frame",
                "Reply to ASTM frames",
                Trigger::AstmFrame,
                None,
                literal_ack(),
            ),
            rule(
                "default-hl7-no-reply-ack",
                "No reply to HL7 acknowledgements",
                Trigger::Hl7 {
                    message_type: "*".to_string(),
                },
                Some(Condition {
                    segment: "MSA".to_string(),
                    field: 1,
                    operator: Operator::Glob,
                    value: "*".to_string(),
                }),
                Action::None,
            ),
            rule(
                "default-cobas-infinity",
                "Reply to Cobas Infinity",
                Trigger::Hl7 {
                    message_type: "OML^O21".to_string(),
                },
                None,
                Action::Hl7Ack {
                    message_type: "ACK^O21".to_string(),
                    code: "CA".to_string(),
                },
            ),
            rule(
                "default-hl7-ack",
                "Reply to HL7 messages",
                Trigger::Hl7 {
                    message_type: "*".to_string(),
                },
                None,
                Action::Template {
                    template_id: "builtin-hl7-ack-o21".to_string(),
                },
            ),
        ],
    }
}

fn is_valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn validate_templates(templates: &[Template]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for (index, template) in templates.iter().enumerate() {
        let label = format!("template #{}", index + 1);
        if template.id.trim().is_empty() {
            return Err(format!("{label}: id must not be empty"));
        }
        if !ids.insert(template.id.as_str()) {
            return Err(format!("{label}: duplicate id \"{}\"", template.id));
        }
        if template.name.trim().is_empty() {
            return Err(format!("{label}: name must not be empty"));
        }
        let mut names = std::collections::HashSet::new();
        for variable in &template.variables {
            if !is_valid_variable_name(&variable.name) {
                return Err(format!(
                    "{label}: invalid variable name \"{}\"",
                    variable.name
                ));
            }
            if !names.insert(variable.name.as_str()) {
                return Err(format!("{label}: duplicate variable \"{}\"", variable.name));
            }
        }
    }
    Ok(())
}

/// Reads the root object. `Ok(None)` when the file does not exist.
fn read_root(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("Cannot read {}: {err}", path.display())),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| format!("{} is not valid JSON: {err}", path.display()))?;
    match value {
        Value::Object(map) => Ok(Some(map)),
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

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Context-free errors: `read_config` adds the file name.
fn parse_auto_reply(value: &Value) -> Result<AutoReplyConfig, String> {
    let config: AutoReplyConfig =
        serde_json::from_value(value.clone()).map_err(|err| err.to_string())?;
    config.validate().map_err(|err| err.to_string())?;
    Ok(config)
}

/// The default rules, or the empty configuration when a template they use is missing (only
/// reachable with a hand-edited file).
fn default_auto_reply_for(templates: &[Template]) -> AutoReplyConfig {
    let defaults = default_auto_reply();
    if RuleSet::compile(&defaults, templates).is_ok() {
        defaults
    } else {
        AutoReplyConfig::default()
    }
}

/// The configuration the app runs with: the parsed file, with defaults for missing keys.
/// This is the single source of truth, for the backend and the frontend.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveConfig {
    pub templates: Vec<Template>,
    pub auto_reply: AutoReplyConfig,
}

/// An `EffectiveConfig` stamped with the configuration revision it was observed at. The revision
/// is a counter owned by the `ConfigStore`: it grows on every successful save, so a consumer can
/// discard a response that arrives after a newer one (IPC responses may be reordered).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RevisionedConfig {
    pub revision: u64,
    #[serde(flatten)]
    pub config: EffectiveConfig,
}

impl std::ops::Deref for RevisionedConfig {
    type Target = EffectiveConfig;
    fn deref(&self) -> &EffectiveConfig {
        &self.config
    }
}

/// A parsed config file: the raw root (to preserve unknown keys) and its effective values.
struct ConfigDocument {
    root: Map<String, Value>,
    effective: EffectiveConfig,
}

/// The one parse path. Missing file or keys give defaults; corrupt values are errors.
/// Template references are not checked here (see `compile_with`).
fn read_config(path: &Path) -> Result<ConfigDocument, String> {
    let root = read_root(path)?.unwrap_or_default();
    let templates = match root.get(TEMPLATES_KEY) {
        None => builtin_templates(),
        Some(value) => parse_templates(value).map_err(|err| {
            format!("Invalid \"{TEMPLATES_KEY}\" in {}: {err}", path.display())
        })?,
    };
    let auto_reply = match root.get(AUTO_REPLY_KEY) {
        None => default_auto_reply_for(&templates),
        Some(value) => parse_auto_reply(value).map_err(|err| {
            format!("Invalid \"{AUTO_REPLY_KEY}\" in {}: {err}", path.display())
        })?,
    };
    Ok(ConfigDocument {
        root,
        effective: EffectiveConfig {
            templates,
            auto_reply,
        },
    })
}

/// A validated configuration and the compiled rule set to apply to the running connections.
#[derive(Debug)]
pub struct Applied {
    pub config: EffectiveConfig,
    pub rules: Arc<RuleSet>,
}

/// Context-free errors.
fn compile_with(
    config: &AutoReplyConfig,
    templates: &[Template],
) -> Result<Arc<RuleSet>, String> {
    RuleSet::compile(config, templates)
        .map(Arc::new)
        .map_err(|err| err.to_string())
}

/// `compile_with` for the config at `path`: the error names the file.
fn compile_at(
    path: &Path,
    config: &AutoReplyConfig,
    templates: &[Template],
) -> Result<Arc<RuleSet>, String> {
    compile_with(config, templates).map_err(|err| {
        format!(
            "Invalid auto reply configuration in {}: {err}",
            path.display()
        )
    })
}

/// Reads and fully validates the configuration (shape and template references).
fn load_from(path: &Path) -> Result<Applied, String> {
    let config = read_config(path)?.effective;
    let rules = compile_at(path, &config.auto_reply, &config.templates)?;
    Ok(Applied { config, rules })
}

/// Replaces the saved rules (the `auto_reply` key is the only one written).
fn save_auto_reply_to(path: &Path, config: &AutoReplyConfig) -> Result<Applied, String> {
    let mut doc = read_config(path)?;
    doc.effective.auto_reply = config.clone();
    write_auto_reply(path, doc)
}

/// Changes only the persisted master switch; the rules are written back untouched.
fn set_auto_reply_enabled_in(path: &Path, enabled: bool) -> Result<Applied, String> {
    let mut doc = read_config(path)?;
    doc.effective.auto_reply.enabled = enabled;
    write_auto_reply(path, doc)
}

fn write_auto_reply(path: &Path, mut doc: ConfigDocument) -> Result<Applied, String> {
    let effective = doc.effective;
    let rules = compile_at(path, &effective.auto_reply, &effective.templates)?;
    doc.root.insert(
        AUTO_REPLY_KEY.to_string(),
        serde_json::to_value(&effective.auto_reply).map_err(|err| err.to_string())?,
    );
    write_root(path, doc.root)?;
    Ok(Applied {
        config: effective,
        rules,
    })
}

/// First rule (enabled or not) whose template is absent from `templates`, as (template id, rule name).
fn first_rule_missing_template(
    config: &AutoReplyConfig,
    templates: &[Template],
) -> Option<(String, String)> {
    config.rules.iter().find_map(|rule| match &rule.action {
        Action::Template { template_id } if !templates.iter().any(|t| &t.id == template_id) => {
            Some((template_id.clone(), rule.name.clone()))
        }
        _ => None,
    })
}

/// Replaces the template library (the `templates` key is the only one written). The `auto_reply`
/// key is never written: while it is missing the default rules stay in effect.
fn save_templates_to(path: &Path, templates: &[Template]) -> Result<Applied, String> {
    validate_templates(templates)?;

    let mut doc = read_config(path)?;
    let key_missing = !doc.root.contains_key(AUTO_REPLY_KEY);
    // Rules (saved or default) that reference a template must keep working with the new library.
    if let Some((template_id, rule_name)) =
        first_rule_missing_template(&doc.effective.auto_reply, templates)
    {
        return Err(format!(
            "Refused: the template \"{template_id}\" is used by the auto reply rule \"{rule_name}\""
        ));
    }
    RuleSet::compile(&doc.effective.auto_reply, templates)
        .map_err(|err| format!("Refused: the auto reply rules would no longer work: {err}"))?;
    doc.effective.templates = templates.to_vec();
    if key_missing {
        // Same as a reload would give for the new library.
        doc.effective.auto_reply = default_auto_reply_for(templates);
    }
    let rules = compile_at(path, &doc.effective.auto_reply, templates)?;
    doc.root.insert(
        TEMPLATES_KEY.to_string(),
        serde_json::to_value(templates).map_err(|err| err.to_string())?,
    );
    write_root(path, doc.root)?;
    Ok(Applied {
        config: doc.effective,
        rules,
    })
}

fn write_root(path: &Path, root: Map<String, Value>) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "simautomate-config-test-{}-{}-{}",
            label,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sample(id: &str) -> Template {
        Template {
            id: id.to_string(),
            name: format!("Name {id}"),
            description: String::new(),
            payload: "<ENQ>{{X}}".to_string(),
            variables: vec![TemplateVariable {
                name: "X".to_string(),
                default: "1".to_string(),
            }],
            autobuild: false,
            autobuild_no_etb: false,
        }
    }

    #[test]
    fn missing_file_seeds_builtins_without_writing() {
        let dir = temp_dir("missing");
        let path = config_path(&dir);
        let loaded = load_from(&path).unwrap().config;
        assert_eq!(loaded.templates, builtin_templates());
        assert!(!dir.exists());
    }

    #[test]
    fn builtin_hl7_frames_are_single_lines_with_explicit_framing() {
        for template in builtin_templates() {
            assert!(!template.payload.contains('\n'));
            assert!(!template.payload.contains('\r'));
            if template.payload.contains("MSH|") {
                assert!(template.payload.starts_with("<VT>MSH|"));
                assert!(template.payload.ends_with("<CR><FS><CR>"));
            }
        }
        assert!(validate_templates(&builtin_templates()).is_ok());
    }

    #[test]
    fn save_creates_directory_and_round_trips() {
        let dir = temp_dir("roundtrip").join("nested");
        let path = config_path(&dir);
        save_auto_reply_to(&path, &AutoReplyConfig::default()).unwrap();
        save_templates_to(&path, &[sample("a"), sample("b")]).unwrap();
        let loaded = load_from(&path).unwrap().config;
        assert_eq!(loaded.templates, vec![sample("a"), sample("b")]);
        // No temp file is left behind.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(leftovers, vec![CONFIG_FILE_NAME.to_string()]);
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn legacy_template_without_autobuild_fields_loads_with_false_defaults() {
        let legacy = r#"{"id":"x","name":"N","payload":"<ENQ>"}"#;
        let t: Template = serde_json::from_str(legacy).unwrap();
        assert!(!t.autobuild);
        assert!(!t.autobuild_no_etb);
        assert!(t.variables.is_empty());
    }

    #[test]
    fn autobuild_flags_round_trip_through_the_file() {
        let dir = temp_dir("autobuild").join("nested");
        let path = config_path(&dir);
        let mut t = sample("a");
        t.autobuild = true;
        t.autobuild_no_etb = true;
        save_auto_reply_to(&path, &AutoReplyConfig::default()).unwrap();
        save_templates_to(&path, &[t.clone(), sample("b")]).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"autobuild\": true"));
        assert!(raw.contains("\"autobuild_no_etb\": true"));
        let loaded = load_from(&path).unwrap().config;
        assert_eq!(loaded.templates, vec![t, sample("b")]);
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn unknown_template_fields_are_still_denied() {
        let bad = r#"{"id":"x","name":"N","payload":"p","autobuildNoEtb":true}"#;
        assert!(serde_json::from_str::<Template>(bad).is_err());
    }

    #[test]
    fn save_preserves_unknown_root_fields_and_replaces_existing_file() {
        let dir = temp_dir("unknown");
        fs::create_dir_all(&dir).unwrap();
        let path = config_path(&dir);
        fs::write(
            &path,
            r#"{"theme":{"dark":true},"future":[1,2],"templates":[]}"#,
        )
        .unwrap();
        save_templates_to(&path, &[sample("a")]).unwrap();
        let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["theme"]["dark"], Value::Bool(true));
        assert_eq!(root["future"], serde_json::json!([1, 2]));
        assert_eq!(root["templates"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_templates_array_is_respected_not_reseeded() {
        let dir = temp_dir("empty");
        let path = config_path(&dir);
        save_auto_reply_to(&path, &AutoReplyConfig::default()).unwrap();
        save_templates_to(&path, &[]).unwrap();
        let loaded = load_from(&path).unwrap().config;
        assert!(loaded.templates.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_builtins_are_not_resurrected() {
        let dir = temp_dir("deleted");
        let path = config_path(&dir);
        let mut kept = builtin_templates();
        kept.remove(0);
        save_templates_to(&path, &kept).unwrap();
        let loaded = load_from(&path).unwrap().config;
        assert_eq!(loaded.templates, kept);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_without_templates_key_seeds_builtins() {
        let dir = temp_dir("nokey");
        fs::create_dir_all(&dir).unwrap();
        let path = config_path(&dir);
        fs::write(&path, r#"{"other":1}"#).unwrap();
        let loaded = load_from(&path).unwrap().config;
        assert_eq!(loaded.templates, builtin_templates());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_files_are_reported_and_never_overwritten() {
        let cases = [
            "{ not json",
            "[1,2,3]",
            r#"{"templates":"nope"}"#,
            r#"{"templates":[{"id":"a"}]}"#,
            r#"{"templates":[{"id":"a","name":"n","payload":"p","extra":1}]}"#,
            r#"{"templates":[{"id":"a","name":"n","payload":"p"},{"id":"a","name":"m","payload":"q"}]}"#,
        ];
        for (index, content) in cases.iter().enumerate() {
            let dir = temp_dir(&format!("bad{index}"));
            fs::create_dir_all(&dir).unwrap();
            let path = config_path(&dir);
            fs::write(&path, content).unwrap();

            assert!(load_from(&path).is_err(), "load case {index}");
            assert!(
                save_templates_to(&path, &[sample("z")]).is_err(),
                "save case {index}"
            );
            assert_eq!(&fs::read_to_string(&path).unwrap(), content);
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn invalid_templates_are_rejected_before_writing() {
        let dir = temp_dir("invalid");
        let path = config_path(&dir);

        let mut no_name = sample("a");
        no_name.name = "  ".to_string();
        assert!(save_templates_to(&path, &[no_name]).is_err());

        let mut bad_var = sample("a");
        bad_var.variables[0].name = "1 bad".to_string();
        assert!(save_templates_to(&path, &[bad_var]).is_err());

        assert!(save_templates_to(&path, &[sample("a"), sample("a")]).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn write_failure_is_reported_and_leaves_existing_file_intact() {
        let dir = temp_dir("blocked");
        fs::create_dir_all(&dir).unwrap();
        // A directory where the file should be makes the replace fail.
        let path = config_path(&dir);
        fs::create_dir_all(&path).unwrap();
        let err = save_templates_to(&path, &[sample("a")]).unwrap_err();
        assert!(!err.is_empty());
        let leftovers = fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1);
        let _ = fs::remove_dir_all(dir);
    }

    // ── auto_reply ────────────────────────────────────────────────────────

    use crate::auto_reply::{Action, Rule, Trigger};

    fn rule_with_template(id: &str, template_id: &str) -> Rule {
        Rule {
            id: id.to_string(),
            name: format!("Rule {id}"),
            enabled: true,
            trigger: Trigger::Hl7 {
                message_type: "ADT^A01".to_string(),
            },
            condition: None,
            action: Action::Template {
                template_id: template_id.to_string(),
            },
            delay_ms: 10,
        }
    }

    fn config_with(rules: Vec<Rule>, enabled: bool) -> AutoReplyConfig {
        AutoReplyConfig { enabled, rules }
    }

    fn setup_file(label: &str, content: &str) -> (PathBuf, PathBuf) {
        let dir = temp_dir(label);
        fs::create_dir_all(&dir).unwrap();
        let path = config_path(&dir);
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn missing_auto_reply_loads_default_rules_without_writing() {
        let dir = temp_dir("ar-missing");
        let path = config_path(&dir);
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.config.auto_reply, default_auto_reply());
        assert!(loaded.config.auto_reply.enabled && loaded.config.auto_reply.rules.len() == 5);
        assert!(loaded.rules.is_enabled());
        assert!(!dir.exists());
    }

    #[test]
    fn default_rules_fall_back_to_empty_when_their_template_is_missing() {
        let (dir, path) = setup_file("ar-default-fallback", r#"{"templates":[]}"#);
        let loaded = load_from(&path).unwrap().config.auto_reply;
        assert!(!loaded.enabled && loaded.rules.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn auto_reply_and_templates_round_trip_and_preserve_each_other_and_unknown_keys() {
        let (dir, path) = setup_file("ar-roundtrip", r#"{"theme":{"dark":true},"future":[1]}"#);
        let config = config_with(vec![rule_with_template("r1", "builtin-hl7-ack-o21")], true);
        // Builtins are effective while the templates key is absent.
        save_auto_reply_to(&path, &config).unwrap();
        let loaded = load_from(&path).unwrap().config;
        assert_eq!(loaded.auto_reply, config);

        save_templates_to(&path, &builtin_templates()).unwrap();
        let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["theme"]["dark"], Value::Bool(true));
        assert_eq!(root["future"], serde_json::json!([1]));
        assert_eq!(root["auto_reply"]["rules"][0]["id"], "r1");
        assert_eq!(root["templates"].as_array().unwrap().len(), 4);
        assert_eq!(load_from(&path).unwrap().config.auto_reply, config);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn no_auto_reply_rule_round_trips_and_needs_no_template() {
        let (dir, path) = setup_file("ar-none", r#"{"templates":[]}"#);
        let mut quiet = rule_with_template("r1", "unused");
        quiet.action = Action::None;
        quiet.delay_ms = 0;
        let config = config_with(vec![quiet], true);
        save_auto_reply_to(&path, &config).unwrap();
        let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            root["auto_reply"]["rules"][0]["action"],
            serde_json::json!({ "type": "none" })
        );
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.config.auto_reply, config);
        assert!(loaded.rules.is_enabled());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_rules_rejects_missing_or_unusable_template_references() {
        let (dir, path) = setup_file("ar-missingref", r#"{"templates":[]}"#);
        let before = fs::read_to_string(&path).unwrap();
        // Empty explicit list: the builtins are not effective any more.
        let err = save_auto_reply_to(
            &path,
            &config_with(vec![rule_with_template("r1", "builtin-hl7-ack-o21")], true),
        )
        .unwrap_err();
        assert!(
            err.contains("Rule r1") && err.contains("does not exist"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);

        let mut bad = sample("bad");
        bad.payload = "{{UNSET}}".to_string();
        bad.variables.clear();
        save_templates_to(&path, &[bad]).unwrap();
        let err = save_auto_reply_to(
            &path,
            &config_with(vec![rule_with_template("r1", "bad")], true),
        )
        .unwrap_err();
        assert!(err.contains("unresolved"), "{err}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn template_edit_or_delete_that_breaks_a_rule_is_refused_and_file_untouched() {
        let (dir, path) = setup_file(
            "ar-refused",
            r#"{"other":1,"auto_reply":{"enabled":false,"rules":[]}}"#,
        );
        save_templates_to(&path, &[sample("a"), sample("b")]).unwrap();
        save_auto_reply_to(
            &path,
            &config_with(vec![rule_with_template("r1", "a")], true),
        )
        .unwrap();
        let before = fs::read_to_string(&path).unwrap();

        // Deleting the referenced template.
        let err = save_templates_to(&path, &[sample("b")]).unwrap_err();
        assert_eq!(
            err,
            "Refused: the template \"a\" is used by the auto reply rule \"Rule r1\""
        );
        // Breaking it: a variable without a default value.
        let mut broken = sample("a");
        broken.variables[0].default = String::new();
        let err = save_templates_to(&path, &[broken, sample("b")]).unwrap_err();
        assert!(
            err.starts_with("Refused: the auto reply rules would no longer work: ")
                && err.contains("Rule r1")
                && err.contains("unresolved"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);

        // A valid edit and the removal of an unreferenced template are fine and return the
        // rule set to apply.
        let mut edited = sample("a");
        edited.payload = "<ACK>".to_string();
        edited.variables.clear();
        let applied = save_templates_to(&path, &[edited]).unwrap();
        assert!(applied.rules.is_enabled());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_a_template_used_by_a_default_rule_names_template_and_rule() {
        let (dir, path) = setup_file("ar-default-delete", r#"{"other":1}"#);
        let before = fs::read_to_string(&path).unwrap();
        let remaining: Vec<Template> = builtin_templates()
            .into_iter()
            .filter(|t| t.id != "builtin-hl7-ack-o21")
            .collect();
        let err = save_templates_to(&path, &remaining).unwrap_err();
        assert_eq!(
            err,
            "Refused: the template \"builtin-hl7-ack-o21\" is used by the auto reply rule \"Reply to HL7 messages\""
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn enabling_changes_only_the_flag() {
        let (dir, path) = setup_file("ar-enable", r#"{"templates":[]}"#);
        save_templates_to(&path, &[sample("a")]).unwrap();
        let config = config_with(vec![rule_with_template("r1", "a")], false);
        save_auto_reply_to(&path, &config).unwrap();

        assert!(set_auto_reply_enabled_in(&path, true)
            .unwrap()
            .rules
            .is_enabled());
        let loaded = load_from(&path).unwrap().config.auto_reply;
        assert!(loaded.enabled);
        assert_eq!(loaded.rules, config.rules);
        assert!(!set_auto_reply_enabled_in(&path, false)
            .unwrap()
            .rules
            .is_enabled());

        // Without an existing key the default rules are persisted with that flag.
        let (dir2, path2) = setup_file("ar-enable2", r#"{"x":1}"#);
        set_auto_reply_enabled_in(&path2, false).unwrap();
        let loaded = load_from(&path2).unwrap().config.auto_reply;
        assert!(!loaded.enabled);
        assert_eq!(loaded.rules, default_auto_reply().rules);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(dir2);
    }

    #[test]
    fn corrupt_files_are_reported_and_never_overwritten_by_rule_operations() {
        let cases = [
            "{ not json",
            "[]",
            r#"{"auto_reply":"nope"}"#,
            r#"{"auto_reply":{"enabled":true,"rules":[{"id":"r"}]}}"#,
            r#"{"auto_reply":{"enabled":true,"rules":[],"extra":1}}"#,
            r#"{"templates":[{"id":"a"}]}"#,
            r#"{"auto_reply":{"enabled":true,"rules":[{"id":"r","name":"n","enabled":true,"trigger":{"type":"astm_enq"},"action":{"type":"template","template_id":"missing"},"delay_ms":0}]}}"#,
        ];
        for (index, content) in cases.iter().enumerate() {
            let (dir, path) = setup_file(&format!("ar-bad{index}"), content);
            assert!(load_from(&path).is_err(), "load {index}");
            // The last case is well-formed but references a missing template: replacing it by a
            // valid configuration is the way to repair it, so only the other cases refuse.
            if index != cases.len() - 1 {
                assert!(
                    save_auto_reply_to(&path, &AutoReplyConfig::default()).is_err(),
                    "save {index}"
                );
            }
            assert!(
                set_auto_reply_enabled_in(&path, true).is_err(),
                "enable {index}"
            );
            assert!(
                save_templates_to(&path, &[sample("z")]).is_err(),
                "templates {index}"
            );
            assert_eq!(&fs::read_to_string(&path).unwrap(), content);
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn invalid_rule_saves_write_nothing() {
        let dir = temp_dir("ar-invalid");
        let path = config_path(&dir);
        let mut rule = rule_with_template("r1", "builtin-hl7-ack-o21");
        rule.delay_ms = 60_001;
        assert!(save_auto_reply_to(&path, &config_with(vec![rule], true)).is_err());
        assert!(!path.exists());
    }
}
