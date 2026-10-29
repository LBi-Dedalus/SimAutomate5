use serde_json::json;

use super::*;

#[test]
fn legacy_template_without_autobuild_fields_loads_with_false_defaults() {
    let legacy = r#"{"id":"x","name":"N","payload":"<ENQ>"}"#;
    let t: Template = serde_json::from_str(legacy).unwrap();
    assert_eq!(t.autobuild, AutobuildMode::DISABLED);
    assert!(t.variables.is_empty());
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
fn template_id_serializes_as_a_plain_string() {
    let id = TemplateId::from("a");
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"a\"");
    assert_eq!(serde_json::from_str::<TemplateId>(&json).unwrap(), id);
}
