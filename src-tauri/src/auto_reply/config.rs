//! Persisted auto reply configuration types and their shape validation.
//!
//! `RawAutoReplyConfig`/`RawAutoReplyRule` hold unchecked data. Parsing them (not done while
//! deserializing the raw types) reports an invalid configuration with the rule it belongs to.
//! `AutoReplyConfig`/`AutoReplyRule` can only be obtained by parsing (or by deserializing,
//! which parses), so a value of these types is always valid.

use serde::{Deserialize, Serialize};

use crate::config_store::TemplateId;

pub const MAX_RULES: usize = 200;
pub const MAX_DELAY_MS: u32 = 60_000;
pub const MAX_LITERAL_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawAutoReplyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<RawAutoReplyRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawAutoReplyRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: AutoReplyTrigger,
    #[serde(default)]
    pub condition: Option<AutoReplyCondition>,
    pub action: AutoReplyAction,
    pub delay_ms: u32,
}

/// A valid configuration; see `RawAutoReplyConfig::parse`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "RawAutoReplyConfig")]
pub struct AutoReplyConfig {
    enabled: bool,
    rules: Vec<AutoReplyRule>,
}

/// A valid rule; see `RawAutoReplyRule::parse`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AutoReplyRule {
    id: String,
    name: String,
    enabled: bool,
    trigger: AutoReplyTrigger,
    condition: Option<AutoReplyCondition>,
    action: AutoReplyAction,
    delay_ms: u32,
}

impl AutoReplyConfig {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn rules(&self) -> &[AutoReplyRule] {
        &self.rules
    }
}

impl AutoReplyRule {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn trigger(&self) -> &AutoReplyTrigger {
        &self.trigger
    }

    pub fn condition(&self) -> Option<&AutoReplyCondition> {
        self.condition.as_ref()
    }

    pub fn action(&self) -> &AutoReplyAction {
        &self.action
    }

    pub fn delay_ms(&self) -> u32 {
        self.delay_ms
    }
}

impl TryFrom<RawAutoReplyConfig> for AutoReplyConfig {
    type Error = String;

    fn try_from(raw: RawAutoReplyConfig) -> Result<Self, String> {
        raw.parse()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutoReplyTrigger {
    /// HL7 message whose type (first two MSH-9 components) matches the exact text or `*` glob.
    Hl7 {
        message_type: String,
    },
    AstmFrame,
    AstmEnq,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoReplyOperator {
    Exact,
    Glob,
}

/// HL7 segment name of a condition (3 upper-case letters/digits, starting with a letter).
/// Serialized as the bare string. Construction is unchecked on purpose: an invalid value is
/// reported by `RawAutoReplyConfig::parse` with the rule it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SegmentId(String);

impl SegmentId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(&self) -> bool {
        let bytes = self.0.as_bytes();
        bytes.len() == 3
            && bytes[0].is_ascii_uppercase()
            && bytes[1..]
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    }
}

impl std::fmt::Display for SegmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for SegmentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SegmentId {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for SegmentId {
    fn from(name: String) -> Self {
        Self(name)
    }
}

/// 1-based HL7 field number of a condition (valid range 1 to 999). Serialized as the bare
/// number; like `SegmentId`, the range is checked by `RawAutoReplyConfig::parse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FieldNumber(u32);

impl FieldNumber {
    pub fn get(self) -> u32 {
        self.0
    }

    pub fn is_valid(self) -> bool {
        (1..=999).contains(&self.0)
    }
}

impl std::fmt::Display for FieldNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<u32> for FieldNumber {
    fn from(number: u32) -> Self {
        Self(number)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoReplyCondition {
    pub segment: SegmentId,
    pub field: FieldNumber,
    pub operator: AutoReplyOperator,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutoReplyAction {
    Template {
        template_id: TemplateId,
    },
    Literal {
        text: String,
    },
    Hl7Ack {
        message_type: String,
        code: String,
    },
    /// Sends nothing and stops the lower-priority rules for the matching message.
    None,
}

// ── Validation ─────────────────────────────────────────────────────────

fn has_control(text: &str) -> bool {
    text.chars().any(|c| c.is_control())
}

fn valid_pattern(text: &str, what: &str) -> Result<(), String> {
    if text.is_empty() || text.chars().count() > 64 {
        return Err(format!("{what} must contain 1 to 64 characters"));
    }
    if has_control(text) || text.contains('|') || text.chars().any(char::is_whitespace) {
        return Err(format!(
            "{what} must not contain spaces, \"|\" or control characters"
        ));
    }
    Ok(())
}

impl RawAutoReplyConfig {
    /// Shape validation (everything that does not need the template library).
    pub fn parse(self) -> Result<AutoReplyConfig, String> {
        if self.rules.len() > MAX_RULES {
            return Err(format!("at most {MAX_RULES} rules are supported"));
        }
        let mut ids = std::collections::HashSet::new();
        let mut rules = Vec::with_capacity(self.rules.len());
        for (index, rule) in self.rules.into_iter().enumerate() {
            let label = if rule.name.trim().is_empty() {
                format!("rule #{}", index + 1)
            } else {
                format!("rule \"{}\"", rule.name.trim())
            };
            let rule = rule.parse().map_err(|err| format!("{label}: {err}"))?;
            if !ids.insert(rule.id.clone()) {
                return Err(format!("{label}: duplicate id \"{}\"", rule.id));
            }
            rules.push(rule);
        }
        Ok(AutoReplyConfig {
            enabled: self.enabled,
            rules,
        })
    }
}

impl RawAutoReplyRule {
    fn parse(self) -> Result<AutoReplyRule, String> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("the id must be 1 to 64 letters, digits, \"-\" or \"_\"".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 100 || has_control(&self.name)
        {
            return Err("the name must contain 1 to 100 characters".into());
        }
        if self.delay_ms > MAX_DELAY_MS {
            return Err(format!("the delay must be between 0 and {MAX_DELAY_MS} ms"));
        }
        match &self.trigger {
            AutoReplyTrigger::Hl7 { message_type } => {
                valid_pattern(message_type, "the message type")?
            }
            AutoReplyTrigger::AstmFrame | AutoReplyTrigger::AstmEnq => {
                if self.condition.is_some() {
                    return Err("conditions are only available for HL7 rules".into());
                }
            }
        }
        if let Some(condition) = &self.condition {
            if !condition.segment.is_valid() {
                return Err("the condition segment must be 3 upper-case letters/digits".into());
            }
            if !condition.field.is_valid() {
                return Err("the condition field number must be between 1 and 999".into());
            }
            if condition.value.is_empty()
                || condition.value.chars().count() > 256
                || has_control(&condition.value)
            {
                return Err("the condition value must contain 1 to 256 characters".into());
            }
        }
        match &self.action {
            AutoReplyAction::None => {}
            AutoReplyAction::Template { template_id } => {
                if template_id.as_str().is_empty() {
                    return Err("choose a template to reply with".into());
                }
            }
            AutoReplyAction::Literal { text } => {
                if text.trim().is_empty() || text.len() > MAX_LITERAL_LEN {
                    return Err(format!(
                        "the literal response must contain 1 to {MAX_LITERAL_LEN} bytes"
                    ));
                }
            }
            AutoReplyAction::Hl7Ack {
                message_type, code, ..
            } => {
                if !matches!(self.trigger, AutoReplyTrigger::Hl7 { .. }) {
                    return Err("a generated HL7 acknowledgement needs an HL7 trigger".into());
                }
                valid_pattern(message_type, "the acknowledgement type")?;
                if message_type.contains('*') {
                    return Err("the acknowledgement type cannot contain \"*\"".into());
                }
                let code = code.as_bytes();
                if code.len() != 2 || !code.iter().all(|b| b.is_ascii_uppercase()) {
                    return Err("the acknowledgement code must be 2 upper-case letters".into());
                }
            }
        }
        Ok(AutoReplyRule {
            id: self.id,
            name: self.name,
            enabled: self.enabled,
            trigger: self.trigger,
            condition: self.condition,
            action: self.action,
            delay_ms: self.delay_ms,
        })
    }
}
