use std::fs;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use serde_json::{json, Value};

use super::defaults::{builtin_templates, default_auto_reply};
use super::document::{config_path, CONFIG_FILE_NAME, TEMP_COUNTER};
use super::validation::validate_templates;
use super::*;
use crate::auto_reply::{AutoReplyAction, AutoReplyTrigger, RawAutoReplyConfig, RawAutoReplyRule};

// The tests do not care about revisions: these shadow the file functions with revision 0.
fn load_from(path: &Path) -> Result<RevisionedConfig, String> {
    super::load_from(path, 0)
}

fn save_auto_reply_to(path: &Path, config: &AutoReplyConfig) -> Result<RevisionedConfig, String> {
    super::save_auto_reply_to(path, 0, config)
}

fn set_auto_reply_enabled_in(path: &Path, enabled: bool) -> Result<RevisionedConfig, String> {
    super::set_auto_reply_enabled_in(path, 0, enabled)
}

fn save_templates_to(path: &Path, templates: &[Template]) -> Result<RevisionedConfig, String> {
    super::save_templates_to(path, 0, templates)
}

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
        id: id.into(),
        name: format!("Name {id}"),
        description: String::new(),
        payload: "<ENQ>{{X}}".to_string(),
        variables: vec![TemplateVariable {
            name: "X".to_string(),
            default: "1".to_string(),
        }],
        autobuild: AutobuildMode::DISABLED,
    }
}

#[test]
fn missing_file_seeds_builtins_without_writing() {
    let dir = temp_dir("missing");
    let path = config_path(&dir);
    let loaded = load_from(&path).unwrap();
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
    let loaded = load_from(&path).unwrap();
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
    assert_eq!(t.autobuild, AutobuildMode::DISABLED);
    assert!(t.variables.is_empty());
}

#[test]
fn autobuild_flags_round_trip_through_the_file() {
    let dir = temp_dir("autobuild").join("nested");
    let path = config_path(&dir);
    let mut t = sample("a");
    t.autobuild = AutobuildMode::Enabled { no_etb: true };
    save_auto_reply_to(&path, &AutoReplyConfig::default()).unwrap();
    save_templates_to(&path, &[t.clone(), sample("b")]).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"autobuild\": true"));
    assert!(raw.contains("\"autobuild_no_etb\": true"));
    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded.templates, vec![t, sample("b")]);
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn template_wire_shape_round_trips_for_every_flag_combination() {
    for (autobuild, no_etb, mode) in [
        (false, false, AutobuildMode::DISABLED),
        (
            false,
            true,
            AutobuildMode::Disabled {
                retained_no_etb: true,
            },
        ),
        (true, false, AutobuildMode::Enabled { no_etb: false }),
        (true, true, AutobuildMode::Enabled { no_etb: true }),
    ] {
        let wire = json!({
            "id": "t",
            "name": "N",
            "description": "d",
            "payload": "<ENQ>",
            "variables": [{"name": "X", "default": "1"}],
            "autobuild": autobuild,
            "autobuild_no_etb": no_etb,
        });
        let template: Template = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(template.autobuild, mode);
        // Same field names, values and order as before the enum.
        let out = serde_json::to_value(&template).unwrap();
        assert_eq!(out, wire);
        // Same field order as before the enum (serde_json::Value sorts keys, so compare text).
        assert_eq!(
            serde_json::to_string(&template).unwrap(),
            format!(
                r#"{{"id":"t","name":"N","description":"d","payload":"<ENQ>","variables":[{{"name":"X","default":"1"}}],"autobuild":{autobuild},"autobuild_no_etb":{no_etb}}}"#
            )
        );
    }
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
    assert_eq!(root["future"], json!([1, 2]));
    assert_eq!(root["templates"].as_array().unwrap().len(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn empty_templates_array_is_respected_not_reseeded() {
    let dir = temp_dir("empty");
    let path = config_path(&dir);
    save_auto_reply_to(&path, &AutoReplyConfig::default()).unwrap();
    save_templates_to(&path, &[]).unwrap();
    let loaded = load_from(&path).unwrap();
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
    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded.templates, kept);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_without_templates_key_seeds_builtins() {
    let dir = temp_dir("nokey");
    fs::create_dir_all(&dir).unwrap();
    let path = config_path(&dir);
    fs::write(&path, r#"{"other":1}"#).unwrap();
    let loaded = load_from(&path).unwrap();
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

fn rule_with_template(id: &str, template_id: &str) -> RawAutoReplyRule {
    RawAutoReplyRule {
        id: id.to_string(),
        name: format!("Rule {id}"),
        enabled: true,
        trigger: AutoReplyTrigger::Hl7 {
            message_type: "ADT^A01".to_string(),
        },
        condition: None,
        action: AutoReplyAction::Template {
            template_id: template_id.into(),
        },
        delay_ms: 10,
    }
}

fn config_with(rules: Vec<RawAutoReplyRule>, enabled: bool) -> AutoReplyConfig {
    RawAutoReplyConfig { enabled, rules }.parse().unwrap()
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
    assert_eq!(loaded.auto_reply, default_auto_reply());
    assert!(loaded.auto_reply.enabled() && loaded.auto_reply.rules().len() == 5);
    assert!(loaded.rules.is_enabled());
    assert!(!dir.exists());
}

#[test]
fn default_rules_fall_back_to_empty_when_their_template_is_missing() {
    let (dir, path) = setup_file("ar-default-fallback", r#"{"templates":[]}"#);
    let loaded = load_from(&path).unwrap().auto_reply;
    assert!(!loaded.enabled() && loaded.rules().is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn auto_reply_and_templates_round_trip_and_preserve_each_other_and_unknown_keys() {
    let (dir, path) = setup_file("ar-roundtrip", r#"{"theme":{"dark":true},"future":[1]}"#);
    let config = config_with(vec![rule_with_template("r1", "builtin-hl7-ack-o21")], true);
    // Builtins are effective while the templates key is absent.
    save_auto_reply_to(&path, &config).unwrap();
    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded.auto_reply, config);

    save_templates_to(&path, &builtin_templates()).unwrap();
    let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(root["theme"]["dark"], Value::Bool(true));
    assert_eq!(root["future"], json!([1]));
    assert_eq!(root["auto_reply"]["rules"][0]["id"], "r1");
    assert_eq!(root["templates"].as_array().unwrap().len(), 4);
    assert_eq!(load_from(&path).unwrap().auto_reply, config);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn no_auto_reply_rule_round_trips_and_needs_no_template() {
    let (dir, path) = setup_file("ar-none", r#"{"templates":[]}"#);
    let mut quiet = rule_with_template("r1", "unused");
    quiet.action = AutoReplyAction::None;
    quiet.delay_ms = 0;
    let config = config_with(vec![quiet], true);
    save_auto_reply_to(&path, &config).unwrap();
    let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        root["auto_reply"]["rules"][0]["action"],
        json!({ "type": "none" })
    );
    let loaded = load_from(&path).unwrap();
    assert_eq!(loaded.auto_reply, config);
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
    let loaded = load_from(&path).unwrap().auto_reply;
    assert!(loaded.enabled());
    assert_eq!(loaded.rules(), config.rules());
    assert!(!set_auto_reply_enabled_in(&path, false)
        .unwrap()
        .rules
        .is_enabled());

    // Without an existing key the default rules are persisted with that flag.
    let (dir2, path2) = setup_file("ar-enable2", r#"{"x":1}"#);
    set_auto_reply_enabled_in(&path2, false).unwrap();
    let loaded = load_from(&path2).unwrap().auto_reply;
    assert!(!loaded.enabled());
    assert_eq!(loaded.rules(), default_auto_reply().rules());
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
fn invalid_rules_cannot_become_a_config() {
    let mut rule = rule_with_template("r1", "builtin-hl7-ack-o21");
    rule.delay_ms = 60_001;
    let raw = RawAutoReplyConfig {
        enabled: true,
        rules: vec![rule],
    };
    assert!(raw.parse().is_err());
}
