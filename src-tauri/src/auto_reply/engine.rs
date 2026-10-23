//! Compiled rule set and evaluation of incoming frames.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::Local;

use super::config::{
    AutoReplyAction, AutoReplyCondition, AutoReplyConfig, AutoReplyRule, AutoReplyTrigger,
};
use super::hl7::{parse_hl7, Hl7Message};
use super::matching::{condition_matches, type_matches};
use super::template::{CompiledTemplate, FramePart};
use crate::config_store::Template;
use crate::message_orchestrator::frames::FrameEvent;
use crate::translate::{ControlToken, ControlTokenEncoder};

#[derive(Debug)]
enum CompiledAutoReplyAction {
    Template(CompiledTemplate),
    Literal(Vec<Vec<u8>>),
    Hl7Ack { message_type: String, code: String },
    None,
}

#[derive(Debug)]
struct CompiledAutoReplyRule {
    name: String,
    enabled: bool,
    trigger: AutoReplyTrigger,
    condition: Option<AutoReplyCondition>,
    action: CompiledAutoReplyAction,
    delay: Duration,
}

/// Immutable, validated and compiled rules, shared by the running connections.
#[derive(Debug)]
pub struct AutoReplyRuleSet {
    enabled: bool,
    rules: Vec<CompiledAutoReplyRule>,
}

#[derive(Debug)]
pub struct AutoReply {
    pub rule_name: String,
    pub delay: Duration,
    pub frames: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub enum AutoReplyOutcome {
    NoMatch,
    Reply(AutoReply),
    /// A "No auto reply" rule matched: nothing is sent and no other rule is tried.
    Suppressed {
        rule_name: String,
    },
    /// A rule matched but its response cannot be produced: nothing is sent and no other rule
    /// is tried.
    Failed {
        rule_name: String,
        reason: String,
    },
}

impl AutoReplyRuleSet {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            rules: Vec::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Compiles a valid configuration against the templates. Disabled rules are compiled too,
    /// so a saved configuration can always be turned on.
    pub fn compile(config: &AutoReplyConfig, templates: &[Template]) -> Result<Self, String> {
        let rules = config
            .rules()
            .iter()
            .map(|rule| CompiledAutoReplyRule::compile(rule, templates))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            enabled: config.enabled(),
            rules,
        })
    }

    /// First enabled matching rule wins. Disabled rules are skipped.
    pub fn evaluate(&self, event: &FrameEvent) -> AutoReplyOutcome {
        if !self.enabled {
            return AutoReplyOutcome::NoMatch;
        }
        let hl7 = match event {
            FrameEvent::Hl7(body) => parse_hl7(body),
            _ => None,
        };
        for rule in self.rules.iter().filter(|rule| rule.enabled) {
            let matched = match (&rule.trigger, event) {
                (AutoReplyTrigger::Hl7 { message_type }, FrameEvent::Hl7(_)) => hl7
                    .as_ref()
                    .map(|message| {
                        type_matches(message_type, &message.message_type())
                            && rule
                                .condition
                                .as_ref()
                                .map_or(true, |condition| condition_matches(condition, message))
                    })
                    .unwrap_or(false),
                (AutoReplyTrigger::AstmFrame, FrameEvent::AstmFrame(_)) => true,
                (AutoReplyTrigger::AstmEnq, FrameEvent::Enq) => true,
                _ => false,
            };
            if !matched {
                continue;
            }
            if matches!(rule.action, CompiledAutoReplyAction::None) {
                return AutoReplyOutcome::Suppressed {
                    rule_name: rule.name.clone(),
                };
            }
            return match rule.respond(hl7.as_ref()) {
                Ok(frames) => AutoReplyOutcome::Reply(AutoReply {
                    rule_name: rule.name.clone(),
                    delay: rule.delay,
                    frames,
                }),
                Err(reason) => AutoReplyOutcome::Failed {
                    rule_name: rule.name.clone(),
                    reason,
                },
            };
        }
        AutoReplyOutcome::NoMatch
    }
}

impl CompiledAutoReplyRule {
    fn compile(rule: &AutoReplyRule, templates: &[Template]) -> Result<Self, String> {
        let label = format!("rule \"{}\"", rule.name().trim());
        let action = match rule.action() {
            AutoReplyAction::Template { template_id } => {
                let template = templates
                    .iter()
                    .find(|template| &template.id == template_id)
                    .ok_or_else(|| {
                        format!("{label} uses the template \"{template_id}\", which does not exist")
                    })?;
                let request_available = matches!(rule.trigger(), AutoReplyTrigger::Hl7 { .. });
                CompiledAutoReplyAction::Template(
                    CompiledTemplate::compile(template, request_available).map_err(|err| {
                        format!(
                            "{label} uses the template \"{}\", which cannot be used: {err}",
                            template.name
                        )
                    })?,
                )
            }
            AutoReplyAction::None => CompiledAutoReplyAction::None,
            AutoReplyAction::Literal { text } => {
                let frames: Vec<Vec<u8>> = text
                    .lines()
                    .map(ControlTokenEncoder::encode_all)
                    .filter(|frame| !frame.is_empty())
                    .collect();
                if frames.is_empty() {
                    return Err(format!("{label}: the literal response is empty"));
                }
                CompiledAutoReplyAction::Literal(frames)
            }
            AutoReplyAction::Hl7Ack { message_type, code } => CompiledAutoReplyAction::Hl7Ack {
                message_type: message_type.clone(),
                code: code.clone(),
            },
        };
        Ok(Self {
            name: rule.name().trim().to_string(),
            enabled: rule.enabled(),
            trigger: rule.trigger().clone(),
            condition: rule.condition().cloned(),
            action,
            delay: Duration::from_millis(rule.delay_ms() as u64),
        })
    }

    fn respond(&self, hl7: Option<&Hl7Message>) -> Result<Vec<Vec<u8>>, String> {
        let now = Local::now().format("%Y%m%d%H%M%S").to_string();
        let control_id = next_control_id();
        match &self.action {
            CompiledAutoReplyAction::None => Ok(Vec::new()),
            CompiledAutoReplyAction::Literal(frames) => Ok(frames.clone()),
            CompiledAutoReplyAction::Template(template) => {
                let mut request_id: Option<Vec<u8>> = None;
                let mut frames = Vec::with_capacity(template.frames.len());
                for parts in &template.frames {
                    let mut frame = Vec::new();
                    for part in parts {
                        match part {
                            FramePart::Bytes(bytes) => frame.extend_from_slice(bytes),
                            FramePart::Now => frame.extend_from_slice(now.as_bytes()),
                            FramePart::ControlId => frame.extend_from_slice(control_id.as_bytes()),
                            FramePart::RequestControlId => {
                                if request_id.is_none() {
                                    request_id = Some(
                                        hl7.and_then(Hl7Message::control_id)
                                            .ok_or_else(|| {
                                                "the request has no usable control id (MSH-10 is empty or invalid)"
                                                    .to_string()
                                            })?
                                            .to_vec(),
                                    );
                                }
                                frame.extend_from_slice(request_id.as_deref().unwrap_or(&[]));
                            }
                        }
                    }
                    frames.push(frame);
                }
                Ok(frames)
            }
            CompiledAutoReplyAction::Hl7Ack { message_type, code } => {
                let request_id = hl7.and_then(Hl7Message::control_id).ok_or_else(|| {
                    "the request has no usable control id (MSH-10 is empty or invalid)".to_string()
                })?;
                if request_id.contains(&b'|') {
                    return Err("the request control id contains \"|\"".into());
                }
                let mut frame = vec![ControlToken::VT.byte()];
                frame.extend_from_slice(
                    format!(
                        "MSH|^~\\&|SIMAUTO|SIM|REMOTE|REMOTE|{now}||{message_type}|{control_id}|P|2.5\rMSA|{code}|"
                    )
                    .as_bytes(),
                );
                // The request id is echoed byte for byte (never decoded, never translated).
                frame.extend_from_slice(request_id);
                frame.push(b'\r');
                frame.push(ControlToken::FS.byte());
                frame.push(ControlToken::CR.byte());
                Ok(vec![frame])
            }
        }
    }
}

static CONTROL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique per response: milliseconds since the epoch followed by a process wide counter.
fn next_control_id() -> String {
    let millis = Local::now().timestamp_millis();
    let counter = CONTROL_COUNTER.fetch_add(1, Ordering::Relaxed) % 10_000;
    format!("{millis}{counter:04}")
}
