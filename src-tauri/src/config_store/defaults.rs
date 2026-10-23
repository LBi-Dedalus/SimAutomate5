//! Built-in defaults: the template library and the auto reply rules used until first saved.

use super::model::{AutobuildMode, Template, TemplateVariable};
use crate::auto_reply::{
    AutoReplyAction, AutoReplyCondition, AutoReplyConfig, AutoReplyOperator, AutoReplyRuleSet,
    AutoReplyTrigger, RawAutoReplyConfig, RawAutoReplyRule,
};

/// Built-in templates. Each HL7 frame is ONE line: the backend sends every real
/// line break as a separate frame, so segments are separated with `<CR>` tokens.
pub(crate) fn builtin_templates() -> Vec<Template> {
    vec![
        Template {
            id: "builtin-hl7-qry-a19".into(),
            name: "HL7 QRY^A19 - Patient query".to_string(),
            description: "MLLP-framed patient query.".to_string(),
            payload: "<VT>MSH|^~\\&|SIMAUTOMATE|LAB|HIS|HOSP|{{NOW}}||QRY^A19|{{CONTROL_ID}}|P|2.4|||AL|NE|<CR>QRD|{{NOW}}|R|I|Q{{CONTROL_ID}}|||1^RD|{{PATIENT_ID}}|DEM|ALL|<CR><FS><CR>".to_string(),
            variables: vec![TemplateVariable {
                name: "PATIENT_ID".to_string(),
                default: String::new(),
            }],
            autobuild: AutobuildMode::DISABLED,
        },
        Template {
            id: "builtin-hl7-ack-o21".into(),
            name: "HL7 ACK^O21 - Acknowledgement".to_string(),
            description: "MLLP-framed acknowledgement of a received message.".to_string(),
            payload: "<VT>MSH|^~\\&|SIMAUTOMATE|LAB|HIS|HOSP|{{NOW}}||ACK^O21|{{CONTROL_ID}}|P|2.4<CR>MSA|AA|{{REQ_CONTROL_ID}}<CR><FS><CR>".to_string(),
            variables: vec![TemplateVariable {
                name: "REQ_CONTROL_ID".to_string(),
                default: String::new(),
            }],
            autobuild: AutobuildMode::DISABLED,
        },
        Template {
            id: "builtin-astm-enq".into(),
            name: "ASTM ENQ".to_string(),
            description: "Starts an ASTM transmission.".to_string(),
            payload: "<ENQ>".to_string(),
            variables: Vec::new(),
            autobuild: AutobuildMode::DISABLED,
        },
        Template {
            id: "builtin-astm-eot".into(),
            name: "ASTM EOT".to_string(),
            description: "Ends an ASTM transmission.".to_string(),
            payload: "<EOT>".to_string(),
            variables: Vec::new(),
            autobuild: AutobuildMode::DISABLED,
        },
    ]
}

/// Default auto reply configuration, used until the `auto_reply` key is first saved.
pub(super) fn default_auto_reply() -> AutoReplyConfig {
    let rule = |id: &str, name: &str, trigger, condition, action| RawAutoReplyRule {
        id: id.to_string(),
        name: name.to_string(),
        enabled: true,
        trigger,
        condition,
        action,
        delay_ms: 0,
    };
    let literal_ack = || AutoReplyAction::Literal {
        text: "<ACK>".to_string(),
    };
    RawAutoReplyConfig {
        enabled: true,
        rules: vec![
            rule(
                "default-astm-enq",
                "Reply to ASTM ENQ",
                AutoReplyTrigger::AstmEnq,
                None,
                literal_ack(),
            ),
            rule(
                "default-astm-frame",
                "Reply to ASTM frames",
                AutoReplyTrigger::AstmFrame,
                None,
                literal_ack(),
            ),
            rule(
                "default-hl7-no-reply-ack",
                "No reply to HL7 acknowledgements",
                AutoReplyTrigger::Hl7 {
                    message_type: "*".to_string(),
                },
                Some(AutoReplyCondition {
                    segment: "MSA".into(),
                    field: 1.into(),
                    operator: AutoReplyOperator::Glob,
                    value: "*".to_string(),
                }),
                AutoReplyAction::None,
            ),
            rule(
                "default-cobas-infinity",
                "Reply to Cobas Infinity",
                AutoReplyTrigger::Hl7 {
                    message_type: "OML^O21".to_string(),
                },
                None,
                AutoReplyAction::Hl7Ack {
                    message_type: "ACK^O21".to_string(),
                    code: "CA".to_string(),
                },
            ),
            rule(
                "default-hl7-ack",
                "Reply to HL7 messages",
                AutoReplyTrigger::Hl7 {
                    message_type: "*".to_string(),
                },
                None,
                AutoReplyAction::Template {
                    template_id: "builtin-hl7-ack-o21".into(),
                },
            ),
        ],
    }
    .parse()
    .expect("the default auto reply rules are valid")
}

/// The default rules, or the empty configuration when a template they use is missing (only
/// reachable with a hand-edited file).
pub(super) fn default_auto_reply_for(templates: &[Template]) -> AutoReplyConfig {
    let defaults = default_auto_reply();
    if AutoReplyRuleSet::compile(&defaults, templates).is_ok() {
        defaults
    } else {
        AutoReplyConfig::default()
    }
}
