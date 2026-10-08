//! Persistence of message templates and auto reply rules in `<app config dir>/config.json`.
//!
//! The file is a JSON object. Only the `templates` and `auto_reply` keys are owned by the app;
//! every other root field is preserved untouched when saving. The file is never overwritten
//! when it exists but cannot be understood (invalid JSON, root that is not an object, or a
//! malformed `templates` / `auto_reply` value): the error is reported instead.
//!
//! Rules reference templates by id, so both keys are cross-validated on every write: a template
//! save is refused when it would break a rule, and a rule save is refused when it references a
//! missing or unusable template. The effective templates are the saved list, or the built-in
//! ones while the `templates` key does not exist. Every successful write returns the validated,
//! compiled rule set the caller must apply to the running connections.
//!
//! Callers must hold the `ConfigLock` for the whole read -> validate -> write sequence.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::auto_reply::{AutoReplyConfig, RuleSet};

pub const CONFIG_FILE_NAME: &str = "config.json";
const TEMPLATES_KEY: &str = "templates";
const AUTO_REPLY_KEY: &str = "auto_reply";

/// Serializes load/save access to the config file (managed separately from `AppState`).
pub struct ConfigLock(pub tokio::sync::Mutex<()>);

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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadedTemplates {
    pub templates: Vec<Template>,
    /// True when the built-in templates were provided because the file or the
    /// `templates` key does not exist yet (nothing was written).
    pub seeded: bool,
}

pub fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// Built-in templates. Each HL7 frame is ONE line: the backend sends every real
/// line break as a separate frame, so segments are separated with `<CR>` tokens.
pub fn builtin_templates() -> Vec<Template> {
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
        },
        Template {
            id: "builtin-astm-enq".to_string(),
            name: "ASTM ENQ".to_string(),
            description: "Starts an ASTM transmission.".to_string(),
            payload: "<ENQ>".to_string(),
            variables: Vec::new(),
        },
        Template {
            id: "builtin-astm-eot".to_string(),
            name: "ASTM EOT".to_string(),
            description: "Ends an ASTM transmission.".to_string(),
            payload: "<EOT>".to_string(),
            variables: Vec::new(),
        },
    ]
}

fn is_valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn validate_templates(templates: &[Template]) -> Result<(), String> {
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

fn parse_templates(value: &Value, path: &Path) -> Result<Vec<Template>, String> {
    let templates: Vec<Template> = serde_json::from_value(value.clone())
        .map_err(|err| format!("Invalid \"templates\" in {}: {err}", path.display()))?;
    validate_templates(&templates)
        .map_err(|err| format!("Invalid \"templates\" in {}: {err}", path.display()))?;
    Ok(templates)
}

pub fn load_templates_from(path: &Path) -> Result<LoadedTemplates, String> {
    let root = read_root(path)?;
    match root.as_ref().and_then(|root| root.get(TEMPLATES_KEY)) {
        None => Ok(LoadedTemplates {
            templates: builtin_templates(),
            seeded: true,
        }),
        Some(value) => Ok(LoadedTemplates {
            templates: parse_templates(value, path)?,
            seeded: false,
        }),
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadedAutoReply {
    pub config: AutoReplyConfig,
    /// False when the file or the `auto_reply` key does not exist yet: the configuration is
    /// then the empty, disabled one (and the frontend may offer to import legacy settings).
    pub present: bool,
}

fn parse_auto_reply(value: &Value, path: &Path) -> Result<AutoReplyConfig, String> {
    let config: AutoReplyConfig = serde_json::from_value(value.clone())
        .map_err(|err| format!("Invalid \"auto_reply\" in {}: {err}", path.display()))?;
    config
        .validate()
        .map_err(|err| format!("Invalid \"auto_reply\" in {}: {err}", path.display()))?;
    Ok(config)
}

/// Saved templates, or the built-in ones while the key does not exist.
fn effective_templates(root: &Map<String, Value>, path: &Path) -> Result<Vec<Template>, String> {
    match root.get(TEMPLATES_KEY) {
        None => Ok(builtin_templates()),
        Some(value) => parse_templates(value, path),
    }
}

/// Existing rules, or the empty configuration when the key is absent.
fn existing_auto_reply(
    root: &Map<String, Value>,
    path: &Path,
) -> Result<Option<AutoReplyConfig>, String> {
    root.get(AUTO_REPLY_KEY)
        .map(|value| parse_auto_reply(value, path))
        .transpose()
}

fn compile_with(
    config: &AutoReplyConfig,
    templates: &[Template],
    path: &Path,
) -> Result<Arc<RuleSet>, String> {
    RuleSet::compile(config, templates)
        .map(Arc::new)
        .map_err(|err| {
            format!(
                "Invalid auto reply configuration in {}: {err}",
                path.display()
            )
        })
}

/// Reads and fully validates the rules (shape and template references).
pub fn load_auto_reply_from(path: &Path) -> Result<LoadedAutoReply, String> {
    let root = read_root(path)?.unwrap_or_default();
    let templates = effective_templates(&root, path)?;
    let present = root.contains_key(AUTO_REPLY_KEY);
    let config = existing_auto_reply(&root, path)?.unwrap_or_default();
    compile_with(&config, &templates, path)?;
    Ok(LoadedAutoReply { config, present })
}

/// The rule set to run with (startup): validated persisted rules, or an error.
pub fn load_rule_set_from(path: &Path) -> Result<Arc<RuleSet>, String> {
    let root = read_root(path)?.unwrap_or_default();
    let templates = effective_templates(&root, path)?;
    let config = existing_auto_reply(&root, path)?.unwrap_or_default();
    compile_with(&config, &templates, path)
}

pub fn save_auto_reply_to(path: &Path, config: &AutoReplyConfig) -> Result<Arc<RuleSet>, String> {
    let mut root = read_root(path)?.unwrap_or_default();
    let templates = effective_templates(&root, path)?;
    // Refuse to replace an auto_reply value we do not understand.
    existing_auto_reply(&root, path)?;
    let rules = compile_with(config, &templates, path)?;
    root.insert(
        AUTO_REPLY_KEY.to_string(),
        serde_json::to_value(config).map_err(|err| err.to_string())?,
    );
    write_root(path, root)?;
    Ok(rules)
}

/// Changes only the persisted master switch; the saved rules are rewritten untouched.
pub fn set_auto_reply_enabled_in(path: &Path, enabled: bool) -> Result<Arc<RuleSet>, String> {
    let mut root = read_root(path)?.unwrap_or_default();
    let templates = effective_templates(&root, path)?;
    let mut config = existing_auto_reply(&root, path)?.unwrap_or_default();
    config.enabled = enabled;
    let rules = compile_with(&config, &templates, path)?;
    root.insert(
        AUTO_REPLY_KEY.to_string(),
        serde_json::to_value(&config).map_err(|err| err.to_string())?,
    );
    write_root(path, root)?;
    Ok(rules)
}

pub fn save_templates_to(path: &Path, templates: &[Template]) -> Result<Arc<RuleSet>, String> {
    validate_templates(templates)?;

    let mut root = read_root(path)?.unwrap_or_default();
    if let Some(existing) = root.get(TEMPLATES_KEY) {
        // Refuse to replace a templates value we do not understand.
        parse_templates(existing, path)?;
    }
    // Rules that reference a template must keep working with the new library.
    let config = existing_auto_reply(&root, path)
        .map_err(|err| format!("Cannot check the auto reply rules before saving: {err}"))?
        .unwrap_or_default();
    let rules = RuleSet::compile(&config, templates)
        .map(Arc::new)
        .map_err(|err| format!("Refused: the change would break the auto reply {err}"))?;
    root.insert(
        TEMPLATES_KEY.to_string(),
        serde_json::to_value(templates).map_err(|err| err.to_string())?,
    );
    write_root(path, root)?;
    Ok(rules)
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
        }
    }

    #[test]
    fn missing_file_seeds_builtins_without_writing() {
        let dir = temp_dir("missing");
        let path = config_path(&dir);
        let loaded = load_templates_from(&path).unwrap();
        assert!(loaded.seeded);
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
        save_templates_to(&path, &[sample("a"), sample("b")]).unwrap();
        let loaded = load_templates_from(&path).unwrap();
        assert!(!loaded.seeded);
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
        save_templates_to(&path, &[]).unwrap();
        let loaded = load_templates_from(&path).unwrap();
        assert!(!loaded.seeded);
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
        let loaded = load_templates_from(&path).unwrap();
        assert_eq!(loaded.templates, kept);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn file_without_templates_key_seeds_builtins() {
        let dir = temp_dir("nokey");
        fs::create_dir_all(&dir).unwrap();
        let path = config_path(&dir);
        fs::write(&path, r#"{"other":1}"#).unwrap();
        let loaded = load_templates_from(&path).unwrap();
        assert!(loaded.seeded);
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

            assert!(load_templates_from(&path).is_err(), "load case {index}");
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
    fn missing_auto_reply_loads_empty_and_disabled_without_writing() {
        let dir = temp_dir("ar-missing");
        let path = config_path(&dir);
        let loaded = load_auto_reply_from(&path).unwrap();
        assert!(!loaded.present);
        assert!(!loaded.config.enabled && loaded.config.rules.is_empty());
        assert!(!load_rule_set_from(&path).unwrap().is_enabled());
        assert!(!dir.exists());
    }

    #[test]
    fn auto_reply_and_templates_round_trip_and_preserve_each_other_and_unknown_keys() {
        let (dir, path) = setup_file("ar-roundtrip", r#"{"theme":{"dark":true},"future":[1]}"#);
        let config = config_with(vec![rule_with_template("r1", "builtin-hl7-ack-o21")], true);
        // Builtins are effective while the templates key is absent.
        save_auto_reply_to(&path, &config).unwrap();
        let loaded = load_auto_reply_from(&path).unwrap();
        assert!(loaded.present);
        assert_eq!(loaded.config, config);

        save_templates_to(&path, &builtin_templates()).unwrap();
        let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["theme"]["dark"], Value::Bool(true));
        assert_eq!(root["future"], serde_json::json!([1]));
        assert_eq!(root["auto_reply"]["rules"][0]["id"], "r1");
        assert_eq!(root["templates"].as_array().unwrap().len(), 4);
        assert_eq!(load_auto_reply_from(&path).unwrap().config, config);
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
        assert_eq!(load_auto_reply_from(&path).unwrap().config, config);
        assert!(load_rule_set_from(&path).unwrap().is_enabled());
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
        let (dir, path) = setup_file("ar-refused", r#"{"other":1}"#);
        save_templates_to(&path, &[sample("a"), sample("b")]).unwrap();
        save_auto_reply_to(
            &path,
            &config_with(vec![rule_with_template("r1", "a")], true),
        )
        .unwrap();
        let before = fs::read_to_string(&path).unwrap();

        // Deleting the referenced template.
        let err = save_templates_to(&path, &[sample("b")]).unwrap_err();
        assert!(err.contains("Rule r1"), "{err}");
        // Breaking it: a variable without a default value.
        let mut broken = sample("a");
        broken.variables[0].default = String::new();
        let err = save_templates_to(&path, &[broken, sample("b")]).unwrap_err();
        assert!(
            err.contains("Rule r1") && err.contains("unresolved"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);

        // A valid edit and the removal of an unreferenced template are fine and return the
        // rule set to apply.
        let mut edited = sample("a");
        edited.payload = "<ACK>".to_string();
        edited.variables.clear();
        let rules = save_templates_to(&path, &[edited]).unwrap();
        assert!(rules.is_enabled());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn enabling_changes_only_the_flag() {
        let (dir, path) = setup_file("ar-enable", r#"{"templates":[]}"#);
        save_templates_to(&path, &[sample("a")]).unwrap();
        let config = config_with(vec![rule_with_template("r1", "a")], false);
        save_auto_reply_to(&path, &config).unwrap();

        assert!(set_auto_reply_enabled_in(&path, true).unwrap().is_enabled());
        let loaded = load_auto_reply_from(&path).unwrap().config;
        assert!(loaded.enabled);
        assert_eq!(loaded.rules, config.rules);
        assert!(!set_auto_reply_enabled_in(&path, false)
            .unwrap()
            .is_enabled());

        // Without an existing key the empty configuration is created with that flag.
        let (dir2, path2) = setup_file("ar-enable2", r#"{"x":1}"#);
        set_auto_reply_enabled_in(&path2, true).unwrap();
        let loaded = load_auto_reply_from(&path2).unwrap();
        assert!(loaded.present && loaded.config.enabled && loaded.config.rules.is_empty());
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
            assert!(load_auto_reply_from(&path).is_err(), "load {index}");
            assert!(load_rule_set_from(&path).is_err(), "startup {index}");
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
