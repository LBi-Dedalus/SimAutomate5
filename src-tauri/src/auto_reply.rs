//! Auto reply rules: typed persisted configuration, validation, compilation and evaluation.
//!
//! The whole behaviour lives in ordered rules (first enabled matching rule wins) plus a global
//! `enabled` switch: there is no hidden default acknowledgement. A rule has a trigger (HL7
//! message type pattern, ASTM frame, ASTM ENQ), an optional HL7 field condition, an action
//! (saved template, literal text, generated HL7 acknowledgement, or "none" to suppress the
//! reply) and a delay.
//!
//! Replies are rendered to raw bytes. Text that comes from the peer (the request control id)
//! is inserted as raw bytes and is NEVER interpreted as control tokens; only text authored in
//! the rule / template goes through `translate::to_bytes`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::config_store::{Template, TemplateVariable};
use crate::frames::FrameEvent;
use crate::translate::{self, ControlToken};

pub const MAX_RULES: usize = 200;
pub const MAX_DELAY_MS: u32 = 60_000;
pub const MAX_LITERAL_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoReplyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    #[serde(default)]
    pub condition: Option<Condition>,
    pub action: Action,
    pub delay_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// HL7 message whose type (first two MSH-9 components) matches the exact text or `*` glob.
    Hl7 {
        message_type: String,
    },
    AstmFrame,
    AstmEnq,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Exact,
    Glob,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub segment: String,
    pub field: u32,
    pub operator: Operator,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Template {
        template_id: String,
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

// â”€â”€ Validation â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

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

impl AutoReplyConfig {
    /// Shape validation (everything that does not need the template library).
    pub fn validate(&self) -> Result<(), String> {
        if self.rules.len() > MAX_RULES {
            return Err(format!("at most {MAX_RULES} rules are supported"));
        }
        let mut ids = std::collections::HashSet::new();
        for (index, rule) in self.rules.iter().enumerate() {
            let label = if rule.name.trim().is_empty() {
                format!("rule #{}", index + 1)
            } else {
                format!("rule \"{}\"", rule.name.trim())
            };
            rule.validate().map_err(|err| format!("{label}: {err}"))?;
            if !ids.insert(rule.id.as_str()) {
                return Err(format!("{label}: duplicate id \"{}\"", rule.id));
            }
        }
        Ok(())
    }
}

impl Rule {
    fn validate(&self) -> Result<(), String> {
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
            Trigger::Hl7 { message_type } => valid_pattern(message_type, "the message type")?,
            Trigger::AstmFrame | Trigger::AstmEnq => {
                if self.condition.is_some() {
                    return Err("conditions are only available for HL7 rules".into());
                }
            }
        }
        if let Some(condition) = &self.condition {
            let segment = condition.segment.as_bytes();
            if segment.len() != 3
                || !segment[0].is_ascii_uppercase()
                || !segment[1..]
                    .iter()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            {
                return Err("the condition segment must be 3 upper-case letters/digits".into());
            }
            if condition.field == 0 || condition.field > 999 {
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
            Action::None => {}
            Action::Template { template_id } => {
                if template_id.is_empty() {
                    return Err("choose a template to reply with".into());
                }
            }
            Action::Literal { text } => {
                if text.trim().is_empty() || text.len() > MAX_LITERAL_LEN {
                    return Err(format!(
                        "the literal response must contain 1 to {MAX_LITERAL_LEN} bytes"
                    ));
                }
            }
            Action::Hl7Ack {
                message_type, code, ..
            } => {
                if !matches!(self.trigger, Trigger::Hl7 { .. }) {
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
        Ok(())
    }
}

// â”€â”€ Template compilation â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[derive(Debug)]
enum Piece {
    Bytes(Vec<u8>),
    Now,
    ControlId,
    RequestControlId,
}

#[derive(Debug)]
struct CompiledTemplate {
    frames: Vec<Vec<Piece>>,
}

enum Token<'a> {
    Text(&'a str),
    Var(&'a str),
}

/// Same grammar as template-core.js: `{{NAME}}`, NAME = [A-Za-z_][A-Za-z0-9_]*.
fn scan_placeholders(source: &str) -> Vec<Token<'_>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'{') {
            let name_start = i + 2;
            let mut end = name_start;
            if end < bytes.len() && (bytes[end].is_ascii_alphabetic() || bytes[end] == b'_') {
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                if bytes.get(end) == Some(&b'}') && bytes.get(end + 1) == Some(&b'}') {
                    if text_start < i {
                        tokens.push(Token::Text(&source[text_start..i]));
                    }
                    tokens.push(Token::Var(&source[name_start..end]));
                    i = end + 2;
                    text_start = i;
                    continue;
                }
            }
        }
        i += 1;
    }
    if text_start < bytes.len() {
        tokens.push(Token::Text(&source[text_start..]));
    }
    tokens
}

fn compile_template(
    template: &Template,
    request_available: bool,
) -> Result<CompiledTemplate, String> {
    // Malformed / stray braces are checked on the text left once the placeholders are removed.
    let leftover: String = scan_placeholders(&template.payload)
        .iter()
        .filter_map(|token| match token {
            Token::Text(text) => Some(*text),
            Token::Var(_) => None,
        })
        .collect();
    if leftover.contains("{{") || leftover.contains("}}") {
        return Err(
            "malformed placeholder: use {{NAME}} with letters, digits and underscores only".into(),
        );
    }

    let mut frames = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for line in template.payload.lines() {
        let mut pieces = Vec::new();
        for token in scan_placeholders(line) {
            match token {
                Token::Text(text) => pieces.push(Piece::Bytes(translate::to_bytes(text))),
                Token::Var("NOW") => pieces.push(Piece::Now),
                Token::Var("CONTROL_ID") => pieces.push(Piece::ControlId),
                Token::Var("REQ_CONTROL_ID") => {
                    if !request_available {
                        return Err(
                            "{{REQ_CONTROL_ID}} is only available for HL7 message rules".into()
                        );
                    }
                    pieces.push(Piece::RequestControlId);
                }
                Token::Var(name) => {
                    let default = template
                        .variables
                        .iter()
                        .find(|variable: &&TemplateVariable| variable.name == name)
                        .map(|variable| variable.default.as_str())
                        .unwrap_or("");
                    if default.contains('\r') || default.contains('\n') {
                        return Err(format!(
                            "the default of {name} contains a line break (it would split the frame)"
                        ));
                    }
                    if default.is_empty() {
                        if !unresolved.iter().any(|n| n == name) {
                            unresolved.push(name.to_string());
                        }
                    } else {
                        pieces.push(Piece::Bytes(translate::to_bytes(default)));
                    }
                }
            }
        }
        if !pieces.is_empty() {
            frames.push(pieces);
        }
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "unresolved variable(s): {} (give them a default value in the template)",
            unresolved.join(", ")
        ));
    }
    if frames.is_empty() {
        return Err("the template payload is empty".into());
    }
    Ok(CompiledTemplate { frames })
}

// â”€â”€ Compiled rule set â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[derive(Debug)]
enum CompiledAction {
    Template(CompiledTemplate),
    Literal(Vec<Vec<u8>>),
    Hl7Ack { message_type: String, code: String },
    None,
}

#[derive(Debug)]
struct CompiledRule {
    name: String,
    enabled: bool,
    trigger: Trigger,
    condition: Option<Condition>,
    action: CompiledAction,
    delay: Duration,
}

/// Immutable, validated and compiled rules, shared by the running connections.
#[derive(Debug)]
pub struct RuleSet {
    enabled: bool,
    rules: Vec<CompiledRule>,
}

#[derive(Debug)]
pub struct Reply {
    pub rule_name: String,
    pub delay: Duration,
    pub frames: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub enum Outcome {
    NoMatch,
    Reply(Reply),
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

impl RuleSet {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            rules: Vec::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Validates the configuration against the templates and compiles it. Disabled rules are
    /// validated too, so a saved configuration can always be turned on.
    pub fn compile(config: &AutoReplyConfig, templates: &[Template]) -> Result<Self, String> {
        config.validate()?;
        let mut rules = Vec::with_capacity(config.rules.len());
        for rule in &config.rules {
            let label = format!("rule \"{}\"", rule.name.trim());
            let action = match &rule.action {
                Action::Template { template_id } => {
                    let template = templates
                        .iter()
                        .find(|template| &template.id == template_id)
                        .ok_or_else(|| {
                            format!(
                                "{label} uses the template \"{template_id}\", which does not exist"
                            )
                        })?;
                    let request_available = matches!(rule.trigger, Trigger::Hl7 { .. });
                    CompiledAction::Template(
                        compile_template(template, request_available).map_err(|err| {
                            format!(
                                "{label} uses the template \"{}\", which cannot be used: {err}",
                                template.name
                            )
                        })?,
                    )
                }
                Action::None => CompiledAction::None,
                Action::Literal { text } => CompiledAction::Literal(
                    text.lines()
                        .map(translate::to_bytes)
                        .filter(|frame| !frame.is_empty())
                        .collect(),
                ),
                Action::Hl7Ack { message_type, code } => CompiledAction::Hl7Ack {
                    message_type: message_type.clone(),
                    code: code.clone(),
                },
            };
            if let CompiledAction::Literal(frames) = &action {
                if frames.is_empty() {
                    return Err(format!("{label}: the literal response is empty"));
                }
            }
            rules.push(CompiledRule {
                name: rule.name.trim().to_string(),
                enabled: rule.enabled,
                trigger: rule.trigger.clone(),
                condition: rule.condition.clone(),
                action,
                delay: Duration::from_millis(rule.delay_ms as u64),
            });
        }
        Ok(Self {
            enabled: config.enabled,
            rules,
        })
    }

    /// First enabled matching rule wins. Disabled rules are skipped.
    pub fn evaluate(&self, event: &FrameEvent) -> Outcome {
        if !self.enabled {
            return Outcome::NoMatch;
        }
        let hl7 = match event {
            FrameEvent::Hl7(body) => parse_hl7(body),
            _ => None,
        };
        for rule in self.rules.iter().filter(|rule| rule.enabled) {
            let matched = match (&rule.trigger, event) {
                (Trigger::Hl7 { message_type }, FrameEvent::Hl7(_)) => hl7
                    .as_ref()
                    .map(|message| {
                        type_matches(message_type, &message.message_type())
                            && rule
                                .condition
                                .as_ref()
                                .map_or(true, |condition| condition_matches(condition, message))
                    })
                    .unwrap_or(false),
                (Trigger::AstmFrame, FrameEvent::AstmFrame(_)) => true,
                (Trigger::AstmEnq, FrameEvent::Enq) => true,
                _ => false,
            };
            if !matched {
                continue;
            }
            if matches!(rule.action, CompiledAction::None) {
                return Outcome::Suppressed {
                    rule_name: rule.name.clone(),
                };
            }
            return match rule.respond(hl7.as_ref()) {
                Ok(frames) => Outcome::Reply(Reply {
                    rule_name: rule.name.clone(),
                    delay: rule.delay,
                    frames,
                }),
                Err(reason) => Outcome::Failed {
                    rule_name: rule.name.clone(),
                    reason,
                },
            };
        }
        Outcome::NoMatch
    }
}

impl CompiledRule {
    fn respond(&self, hl7: Option<&Hl7Message>) -> Result<Vec<Vec<u8>>, String> {
        let now = Local::now().format("%Y%m%d%H%M%S").to_string();
        let control_id = next_control_id();
        match &self.action {
            CompiledAction::None => Ok(Vec::new()),
            CompiledAction::Literal(frames) => Ok(frames.clone()),
            CompiledAction::Template(template) => {
                let mut request_id: Option<Vec<u8>> = None;
                let mut frames = Vec::with_capacity(template.frames.len());
                for pieces in &template.frames {
                    let mut frame = Vec::new();
                    for piece in pieces {
                        match piece {
                            Piece::Bytes(bytes) => frame.extend_from_slice(bytes),
                            Piece::Now => frame.extend_from_slice(now.as_bytes()),
                            Piece::ControlId => frame.extend_from_slice(control_id.as_bytes()),
                            Piece::RequestControlId => {
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
            CompiledAction::Hl7Ack { message_type, code } => {
                let request_id = hl7.and_then(Hl7Message::control_id).ok_or_else(|| {
                    "the request has no usable control id (MSH-10 is empty or invalid)".to_string()
                })?;
                if request_id.contains(&b'|') {
                    return Err("the request control id contains \"|\"".into());
                }
                let mut frame = vec![ControlToken::VT as u8];
                frame.extend_from_slice(
                    format!(
                        "MSH|^~\\&|SIMAUTO|SIM|REMOTE|REMOTE|{now}||{message_type}|{control_id}|P|2.5\rMSA|{code}|"
                    )
                    .as_bytes(),
                );
                // The request id is echoed byte for byte (never decoded, never translated).
                frame.extend_from_slice(request_id);
                frame.push(b'\r');
                frame.push(ControlToken::FS as u8);
                frame.push(ControlToken::CR as u8);
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

// â”€â”€ Matching helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Fully anchored, case-sensitive match where `*` matches any run of characters (even empty).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// A pattern containing `*` is "generic": it never matches an HL7 acknowledgement (message type
/// `ACK`) unless it deliberately starts with `ACK`, so a catch-all rule cannot answer an
/// acknowledgement with another one forever. Exact patterns are always deliberate.
fn type_matches(pattern: &str, message_type: &str) -> bool {
    if pattern.contains('*') {
        let first = message_type.split('^').next().unwrap_or("");
        if first == "ACK" && !pattern.starts_with("ACK") {
            return false;
        }
        glob_match(pattern, message_type)
    } else {
        pattern == message_type
    }
}

fn condition_matches(condition: &Condition, message: &Hl7Message) -> bool {
    match message.field(&condition.segment, condition.field) {
        None => false,
        Some(value) => match condition.operator {
            Operator::Exact => value == condition.value,
            Operator::Glob => glob_match(&condition.value, &value),
        },
    }
}

struct Hl7Message {
    field_sep: char,
    component_sep: char,
    segments: Vec<String>,
    /// MSH-10 exactly as received (the decoded `segments` are lossy for non UTF-8 bytes).
    raw_control_id: Option<Vec<u8>>,
}

/// MSH-10 taken straight from the received bytes, only when non empty and free of control bytes.
fn raw_control_id(body: &[u8], field_sep: u8) -> Option<Vec<u8>> {
    let msh = body
        .split(|b| *b == b'\r' || *b == b'\n')
        .find(|segment| !segment.is_empty())?;
    let id = msh.split(|b| *b == field_sep).nth(9)?;
    if id.is_empty() || id.iter().any(|b| *b < 0x20 || *b == 0x7f) {
        return None;
    }
    if let Ok(text) = std::str::from_utf8(id) {
        if text.chars().any(|c| c.is_control()) {
            return None;
        }
    }
    Some(id.to_vec())
}

fn parse_hl7(body: &[u8]) -> Option<Hl7Message> {
    let text = String::from_utf8_lossy(body);
    let segments: Vec<String> = text
        .split(['\r', '\n'])
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();
    let msh = segments.first()?;
    if !msh.starts_with("MSH") {
        return None;
    }
    let field_sep = msh.chars().nth(3)?;
    if !field_sep.is_ascii() || field_sep.is_control() || field_sep.is_alphanumeric() {
        return None;
    }
    let component_sep = msh
        .split(field_sep)
        .nth(1)
        .and_then(|encoding| encoding.chars().next())
        .unwrap_or('^');
    let raw_control_id = raw_control_id(body, field_sep as u8);
    Some(Hl7Message {
        field_sep,
        component_sep,
        segments,
        raw_control_id,
    })
}

impl Hl7Message {
    /// Field `n` of the first segment named `segment`. MSH-1 is the field separator itself,
    /// so MSH-n is the n-th separated item (MSH-2 are the encoding characters).
    fn field(&self, segment: &str, n: u32) -> Option<String> {
        let n = n as usize;
        for line in &self.segments {
            let mut parts = line.split(self.field_sep);
            if parts.next() != Some(segment) {
                continue;
            }
            if segment == "MSH" {
                if n == 1 {
                    return Some(self.field_sep.to_string());
                }
                return line.split(self.field_sep).nth(n - 1).map(str::to_string);
            }
            return line.split(self.field_sep).nth(n).map(str::to_string);
        }
        None
    }

    /// First two MSH-9 components joined with `^` (`ORU^R01^ORU_R01` gives `ORU^R01`).
    fn message_type(&self) -> String {
        let value = self.field("MSH", 9).unwrap_or_default();
        value
            .split(self.component_sep)
            .take(2)
            .collect::<Vec<_>>()
            .join("^")
    }

    /// MSH-10 as received (raw bytes), only when non empty and free of control bytes.
    fn control_id(&self) -> Option<&[u8]> {
        self.raw_control_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_store::builtin_templates;

    fn rule(id: &str, trigger: Trigger, condition: Option<Condition>, action: Action) -> Rule {
        Rule {
            id: id.to_string(),
            name: format!("Rule {id}"),
            enabled: true,
            trigger,
            condition,
            action,
            delay_ms: 0,
        }
    }

    fn hl7(pattern: &str) -> Trigger {
        Trigger::Hl7 {
            message_type: pattern.to_string(),
        }
    }

    fn literal(text: &str) -> Action {
        Action::Literal {
            text: text.to_string(),
        }
    }

    fn tpl(id: &str, payload: &str, defaults: &[(&str, &str)]) -> Template {
        Template {
            id: id.to_string(),
            name: format!("T {id}"),
            description: String::new(),
            payload: payload.to_string(),
            variables: defaults
                .iter()
                .map(|(name, default)| TemplateVariable {
                    name: name.to_string(),
                    default: default.to_string(),
                })
                .collect(),
        }
    }

    fn compile(rules: Vec<Rule>, templates: &[Template]) -> Result<RuleSet, String> {
        RuleSet::compile(
            &AutoReplyConfig {
                enabled: true,
                rules,
            },
            templates,
        )
    }

    fn msg(msh9: &str, id: &str, extra: &str) -> FrameEvent {
        FrameEvent::Hl7(
            format!("MSH|^~\\&|S|F|R|F|20260101||{msh9}|{id}|P|2.4\r{extra}").into_bytes(),
        )
    }

    fn reply_text(outcome: Outcome) -> String {
        match outcome {
            Outcome::Reply(reply) => reply
                .frames
                .iter()
                .map(|f| translate::to_human_readable(f))
                .collect::<Vec<_>>()
                .join("|FRAME|"),
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    fn none_when(id: &str, trigger: Trigger, condition: Option<Condition>) -> Rule {
        rule(id, trigger, condition, Action::None)
    }

    fn qrd8(value: &str) -> Option<Condition> {
        Some(Condition {
            segment: "QRD".into(),
            field: 8,
            operator: Operator::Glob,
            value: value.into(),
        })
    }

    #[test]
    fn no_auto_reply_suppresses_and_stops_lower_rules() {
        let set = compile(
            vec![
                none_when("quiet", hl7("QRY*"), qrd8("AAZ*")),
                rule(
                    "ack",
                    hl7("*"),
                    None,
                    Action::Hl7Ack {
                        message_type: "ACK".into(),
                        code: "AA".into(),
                    },
                ),
                literal_rule("lit"),
            ],
            &[],
        )
        .unwrap();
        // Matching message: suppressed, with no frames, no failure and no later rule.
        match set.evaluate(&msg("QRY^A19", "X1", "QRD|1|1|1|1|1|1|1|AAZ9\r")) {
            Outcome::Suppressed { rule_name } => assert_eq!(rule_name, "Rule quiet"),
            other => panic!("expected suppression, got {other:?}"),
        }
        // Condition not matching: falls through to the next rule.
        assert!(
            reply_text(set.evaluate(&msg("QRY^A19", "X2", "QRD|1|1|1|1|1|1|1|OTHER\r")))
                .contains("MSA|AA|X2")
        );
        // Missing/invalid MSH-10 is irrelevant for a suppressing rule.
        let no_id = FrameEvent::Hl7(
            b"MSH|^~\\&|S|F|R|F|2026||QRY^A19||P|2.4\rQRD|1|1|1|1|1|1|1|AAZ1\r".to_vec(),
        );
        assert!(matches!(set.evaluate(&no_id), Outcome::Suppressed { .. }));
    }

    fn literal_rule(id: &str) -> Rule {
        rule(id, hl7("*"), None, literal("LATE"))
    }

    #[test]
    fn disabled_no_auto_reply_rule_is_skipped() {
        let mut quiet = none_when("quiet", hl7("*"), None);
        quiet.enabled = false;
        let set = compile(vec![quiet, literal_rule("lit")], &[]).unwrap();
        assert_eq!(reply_text(set.evaluate(&msg("ADT^A01", "1", ""))), "LATE");
    }

    #[test]
    fn no_auto_reply_works_for_astm_triggers_and_needs_no_template() {
        let set = compile(
            vec![
                none_when("enq", Trigger::AstmEnq, None),
                none_when("frame", Trigger::AstmFrame, None),
                rule("late", Trigger::AstmEnq, None, literal("<ACK>")),
            ],
            &[],
        )
        .unwrap();
        assert!(matches!(
            set.evaluate(&FrameEvent::Enq),
            Outcome::Suppressed { .. }
        ));
        assert!(matches!(
            set.evaluate(&FrameEvent::AstmFrame(b"1H|".to_vec())),
            Outcome::Suppressed { .. }
        ));
        // Master switch off: nothing evaluated at all.
        let off = RuleSet::compile(
            &AutoReplyConfig {
                enabled: false,
                rules: vec![none_when("enq", Trigger::AstmEnq, None)],
            },
            &[],
        )
        .unwrap();
        assert!(matches!(off.evaluate(&FrameEvent::Enq), Outcome::NoMatch));
    }

    #[test]
    fn no_auto_reply_serializes_as_type_none() {
        let json = serde_json::to_value(Action::None).unwrap();
        assert_eq!(json, serde_json::json!({ "type": "none" }));
        let back: Action = serde_json::from_value(json).unwrap();
        assert_eq!(back, Action::None);
    }

    #[test]
    fn glob_is_anchored_case_sensitive_and_only_star_is_special() {
        assert!(glob_match("AAZ*", "AAZ123"));
        assert!(glob_match("AAZ*", "AAZ"));
        assert!(!glob_match("AAZ*", "XAAZ1"));
        assert!(!glob_match("AAZ*", "aaz1"));
        assert!(glob_match("*", ""));
        assert!(glob_match("A*B*C", "AxxBxxC"));
        assert!(!glob_match("A*B", "AxxBx"));
        assert!(!glob_match("A?C", "ABC"));
        assert!(glob_match("A?C", "A?C"));
        assert!(glob_match("QRY^A19", "QRY^A19"));
    }

    #[test]
    fn first_matching_rule_wins_and_otherwise_rule_comes_last() {
        let templates = vec![
            tpl("adr", "<VT>ADR {{REQ_CONTROL_ID}}<FS><CR>", &[]),
            tpl("nf", "NOTFOUND", &[]),
        ];
        let cond = Condition {
            segment: "QRD".into(),
            field: 8,
            operator: Operator::Glob,
            value: "AAZ*".into(),
        };
        let set = compile(
            vec![
                rule(
                    "1",
                    hl7("QRY^A19"),
                    Some(cond),
                    Action::Template {
                        template_id: "adr".into(),
                    },
                ),
                rule(
                    "2",
                    hl7("QRY^A19"),
                    None,
                    Action::Template {
                        template_id: "nf".into(),
                    },
                ),
            ],
            &templates,
        )
        .unwrap();
        let hit = msg("QRY^A19", "C1", "QRD|1|2|3|4|5|6|7|AAZ99|9\r");
        assert_eq!(reply_text(set.evaluate(&hit)), "<VT>ADR C1<FS><CR>");
        let miss = msg("QRY^A19", "C2", "QRD|1|2|3|4|5|6|7|ZZZ|9\r");
        assert_eq!(reply_text(set.evaluate(&miss)), "NOTFOUND");
        assert!(matches!(
            set.evaluate(&msg("ADT^A01", "C3", "")),
            Outcome::NoMatch
        ));
    }

    #[test]
    fn disabled_rules_and_disabled_master_never_reply() {
        let mut disabled = rule("1", hl7("*"), None, literal("<ACK>"));
        disabled.enabled = false;
        let fallback = rule("2", hl7("*"), None, literal("SECOND"));
        let set = compile(vec![disabled, fallback], &[]).unwrap();
        assert_eq!(reply_text(set.evaluate(&msg("ADT^A01", "1", ""))), "SECOND");

        let off = RuleSet::compile(
            &AutoReplyConfig {
                enabled: false,
                rules: vec![rule("1", hl7("*"), None, literal("X"))],
            },
            &[],
        )
        .unwrap();
        assert!(matches!(
            off.evaluate(&msg("ADT^A01", "1", "")),
            Outcome::NoMatch
        ));
        assert!(matches!(
            RuleSet::disabled().evaluate(&FrameEvent::Enq),
            Outcome::NoMatch
        ));
    }

    #[test]
    fn type_uses_two_components_and_optional_structure() {
        let set = compile(vec![rule("1", hl7("ORU^R01"), None, literal("OK"))], &[]).unwrap();
        assert_eq!(
            reply_text(set.evaluate(&msg("ORU^R01^ORU_R01", "1", ""))),
            "OK"
        );
        assert_eq!(reply_text(set.evaluate(&msg("ORU^R01", "1", ""))), "OK");
        assert!(matches!(
            set.evaluate(&msg("ORU^R02", "1", "")),
            Outcome::NoMatch
        ));
        assert!(matches!(
            set.evaluate(&msg("ORU", "1", "")),
            Outcome::NoMatch
        ));
        let glob = compile(vec![rule("1", hl7("ORU*"), None, literal("OK"))], &[]).unwrap();
        assert_eq!(
            reply_text(glob.evaluate(&msg("ORU^R01^ORU_R01", "1", ""))),
            "OK"
        );
    }

    #[test]
    fn generic_patterns_do_not_answer_acknowledgements_but_explicit_ones_can() {
        let generic = compile(vec![rule("1", hl7("*"), None, literal("X"))], &[]).unwrap();
        assert!(matches!(
            generic.evaluate(&msg("ACK^A01", "1", "MSA|AA|1\r")),
            Outcome::NoMatch
        ));
        assert!(matches!(
            generic.evaluate(&msg("ACK", "1", "")),
            Outcome::NoMatch
        ));
        let explicit = compile(vec![rule("1", hl7("ACK^A01"), None, literal("X"))], &[]).unwrap();
        assert_eq!(reply_text(explicit.evaluate(&msg("ACK^A01", "1", ""))), "X");
        let deliberate = compile(vec![rule("1", hl7("ACK*"), None, literal("X"))], &[]).unwrap();
        assert_eq!(
            reply_text(deliberate.evaluate(&msg("ACK^A01", "1", ""))),
            "X"
        );
    }

    #[test]
    fn custom_delimiters_and_msh_offsets_are_honoured() {
        let event = FrameEvent::Hl7(
            b"MSH#*~\\&#S#F#R#F#2026##QRY*A19*QRY_A19#CTL#P#2.4\rQRD#1#2#3#4#5#6#7#AAZ*9\r"
                .to_vec(),
        );
        let cond = |segment: &str, field: u32, value: &str| Condition {
            segment: segment.into(),
            field,
            operator: Operator::Exact,
            value: value.into(),
        };
        let set = compile(
            vec![rule(
                "0",
                hl7("QRY^A19"),
                Some(cond("MSH", 1, "#")),
                literal("SEP"),
            )],
            &[],
        )
        .unwrap();
        assert_eq!(reply_text(set.evaluate(&event)), "SEP");
        let set = compile(
            vec![rule(
                "0",
                hl7("QRY^A19"),
                Some(cond("MSH", 2, "*~\\&")),
                literal("ENC"),
            )],
            &[],
        )
        .unwrap();
        assert_eq!(reply_text(set.evaluate(&event)), "ENC");
        let set = compile(
            vec![rule(
                "0",
                hl7("QRY^A19"),
                Some(cond("MSH", 10, "CTL")),
                literal("MSH10"),
            )],
            &[],
        )
        .unwrap();
        assert_eq!(reply_text(set.evaluate(&event)), "MSH10");
        // Field values are compared raw: components are part of the value.
        let set = compile(
            vec![rule(
                "0",
                hl7("QRY^A19"),
                Some(cond("QRD", 8, "AAZ*9")),
                literal("RAW"),
            )],
            &[],
        )
        .unwrap();
        assert_eq!(reply_text(set.evaluate(&event)), "RAW");
        // Missing segment or field never matches.
        let set = compile(
            vec![rule(
                "0",
                hl7("QRY^A19"),
                Some(cond("PID", 1, "x")),
                literal("NO"),
            )],
            &[],
        )
        .unwrap();
        assert!(matches!(set.evaluate(&event), Outcome::NoMatch));
    }

    #[test]
    fn astm_rules_match_frames_and_enq_only() {
        let set = compile(
            vec![
                rule("1", Trigger::AstmEnq, None, literal("<ACK>")),
                rule("2", Trigger::AstmFrame, None, literal("<ACK>")),
            ],
            &[],
        )
        .unwrap();
        assert!(matches!(set.evaluate(&FrameEvent::Enq), Outcome::Reply(_)));
        assert!(matches!(
            set.evaluate(&FrameEvent::AstmFrame(b"\x021H\r\x0300\r\n".to_vec())),
            Outcome::Reply(_)
        ));
        assert!(matches!(
            set.evaluate(&msg("ADT^A01", "1", "")),
            Outcome::NoMatch
        ));
        assert!(matches!(
            set.evaluate(&FrameEvent::Malformed("x")),
            Outcome::NoMatch
        ));
    }

    #[test]
    fn generated_ack_uses_rule_type_code_and_request_id() {
        let set = compile(
            vec![rule(
                "1",
                hl7("ORU^R01"),
                None,
                Action::Hl7Ack {
                    message_type: "ACK^R01".into(),
                    code: "AE".into(),
                },
            )],
            &[],
        )
        .unwrap();
        let text = reply_text(set.evaluate(&msg("ORU^R01", "abc-987", "")));
        assert!(text.starts_with("<VT>MSH|^~\\&|"));
        assert!(text.contains("||ACK^R01|"));
        assert!(text.contains("<CR>MSA|AE|abc-987<CR>"));
        assert!(text.ends_with("<FS><CR>"));
    }

    #[test]
    fn template_variables_come_from_context_defaults_and_request() {
        let templates = vec![tpl(
            "t",
            "<VT>MSH|{{NOW}}|{{CONTROL_ID}}|{{REQ_CONTROL_ID}}|{{WHO}}<CR>MSA|{{REQ_CONTROL_ID}}<FS><CR>",
            &[("REQ_CONTROL_ID", "OLD"), ("WHO", "<ETX>x")],
        )];
        let set = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Template {
                    template_id: "t".into(),
                },
            )],
            &templates,
        )
        .unwrap();
        let first = reply_text(set.evaluate(&msg("ADT^A01", "REQ1", "")));
        let second = reply_text(set.evaluate(&msg("ADT^A01", "REQ2", "")));
        assert!(
            first.contains("|REQ1|<ETX>x<CR>MSA|REQ1<FS><CR>"),
            "{first}"
        );
        assert!(!first.contains("OLD"));
        let id = |text: &str| text.split('|').nth(2).unwrap().to_string();
        assert_ne!(id(&first), id(&second));
        let now = first.split('|').nth(1).unwrap();
        assert_eq!(now.len(), 14);
        assert!(now.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn request_data_is_never_translated_into_framing() {
        let templates = vec![tpl("t", "<VT>A{{REQ_CONTROL_ID}}B<FS><CR>", &[])];
        let set = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Template {
                    template_id: "t".into(),
                },
            )],
            &templates,
        )
        .unwrap();
        // Literal text that looks like control tokens stays literal wire bytes.
        let outcome = set.evaluate(&msg("ADT^A01", "x<FS><CR><VT>y", ""));
        match outcome {
            Outcome::Reply(reply) => {
                assert_eq!(reply.frames.len(), 1);
                assert_eq!(reply.frames[0], b"\x0bAx<FS><CR><VT>yB\x1c\r".to_vec());
            }
            other => panic!("{other:?}"),
        }
        // Raw control bytes in the request id are refused: nothing is sent.
        let bad = FrameEvent::Hl7(b"MSH|^~\\&|S|F|R|F|1||ADT^A01|a\x1cb|P|2.4\r".to_vec());
        assert!(matches!(set.evaluate(&bad), Outcome::Failed { .. }));
        for id in [""] {
            let empty = msg("ADT^A01", id, "");
            assert!(matches!(set.evaluate(&empty), Outcome::Failed { .. }));
        }
        // A failing rule does not fall through to a lower matching rule.
        let both = compile(
            vec![
                rule(
                    "1",
                    hl7("*"),
                    None,
                    Action::Template {
                        template_id: "t".into(),
                    },
                ),
                rule("2", hl7("*"), None, literal("FALLBACK")),
            ],
            &templates,
        )
        .unwrap();
        assert!(matches!(
            both.evaluate(&msg("ADT^A01", "", "")),
            Outcome::Failed { .. }
        ));
        let ack = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Hl7Ack {
                    message_type: "ACK".into(),
                    code: "AA".into(),
                },
            )],
            &[],
        )
        .unwrap();
        // With another field separator "|" is data, but it would break the generated ACK.
        let odd = FrameEvent::Hl7(b"MSH#^~\\&#S#F#R#F#1##ADT^A01#a|b#P#2.4\r".to_vec());
        assert!(matches!(ack.evaluate(&odd), Outcome::Failed { .. }));
    }

    #[test]
    fn request_control_id_is_echoed_byte_for_byte_even_when_not_utf8() {
        let templates = [tpl("t", "A{{REQ_CONTROL_ID}}B", &[])];
        let template_set = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Template {
                    template_id: "t".into(),
                },
            )],
            &templates,
        )
        .unwrap();
        let ack_set = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Hl7Ack {
                    message_type: "ACK".into(),
                    code: "AA".into(),
                },
            )],
            &[],
        )
        .unwrap();
        let ids: Vec<Vec<u8>> = vec![
            b"ID-\xe9".to_vec(),
            b"\xff\xfe\x80Z".to_vec(),
            b"A\xc3\xa9\xe2\x82\xac".to_vec(), // valid UTF-8 stays as is
            b"<FS><CR>x".to_vec(),             // textual tokens are data, not framing
            b"{{NOW}}".to_vec(),
        ];
        for id in ids {
            let mut body = b"MSH|^~\\&|S|F|R|F|1||ADT^A01|".to_vec();
            body.extend_from_slice(&id);
            body.extend_from_slice(b"|P|2.4\rPID|1\r");
            let event = FrameEvent::Hl7(body);
            match template_set.evaluate(&event) {
                Outcome::Reply(reply) => {
                    let mut expected = b"A".to_vec();
                    expected.extend_from_slice(&id);
                    expected.push(b'B');
                    assert_eq!(reply.frames, vec![expected], "template {id:?}");
                }
                other => panic!("{other:?}"),
            }
            match ack_set.evaluate(&event) {
                Outcome::Reply(reply) => {
                    let frame = &reply.frames[0];
                    let mut tail = b"|AA|".to_vec();
                    tail.extend_from_slice(&id);
                    tail.extend_from_slice(b"\r\x1c\r");
                    assert!(frame.windows(tail.len()).any(|w| w == tail), "ack {id:?}");
                }
                other => panic!("{other:?}"),
            }
        }
        // Custom separators with a non UTF-8 id.
        let custom = FrameEvent::Hl7(b"MSH#^~\\&#S#F#R#F#1##ADT^A01#K\xe9Y#P#2.4\r".to_vec());
        match template_set.evaluate(&custom) {
            Outcome::Reply(reply) => assert_eq!(reply.frames, vec![b"AK\xe9YB".to_vec()]),
            other => panic!("{other:?}"),
        }
        // Raw control bytes (also inside otherwise non UTF-8 ids) or a missing MSH-10: no reply.
        for body in [
            &b"MSH|^~\\&|S|F|R|F|1||ADT^A01|a\x1c\xe9|P|2.4\r"[..],
            b"MSH|^~\\&|S|F|R|F|1||ADT^A01|\xe9\x7f|P|2.4\r",
            b"MSH|^~\\&|S|F|R|F|1||ADT^A01",
        ] {
            let event = FrameEvent::Hl7(body.to_vec());
            assert!(matches!(
                template_set.evaluate(&event),
                Outcome::Failed { .. }
            ));
            assert!(matches!(ack_set.evaluate(&event), Outcome::Failed { .. }));
        }
    }

    #[test]
    fn template_problems_are_rejected_at_compile_time_naming_the_rule() {
        let cases: Vec<(Template, &str)> = vec![
            (tpl("t", "A {{BROKEN", &[]), "malformed"),
            (tpl("t", "A }} B", &[]), "malformed"),
            (tpl("t", "A {{X}}", &[]), "unresolved"),
            (tpl("t", "A {{X}}", &[("X", "")]), "unresolved"),
            (tpl("t", "A {{X}}", &[("X", "a\nb")]), "line break"),
            (tpl("t", "", &[]), "empty"),
        ];
        for (template, expected) in cases {
            let err = compile(
                vec![rule(
                    "1",
                    hl7("*"),
                    None,
                    Action::Template {
                        template_id: "t".into(),
                    },
                )],
                &[template],
            )
            .unwrap_err();
            assert!(err.contains("Rule 1") && err.contains(expected), "{err}");
        }
        let err = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Template {
                    template_id: "gone".into(),
                },
            )],
            &[],
        )
        .unwrap_err();
        assert!(
            err.contains("Rule 1") && err.contains("does not exist"),
            "{err}"
        );
        // REQ_CONTROL_ID needs an HL7 request.
        let err = compile(
            vec![rule(
                "1",
                Trigger::AstmEnq,
                None,
                Action::Template {
                    template_id: "t".into(),
                },
            )],
            &[tpl("t", "{{REQ_CONTROL_ID}}", &[])],
        )
        .unwrap_err();
        assert!(err.contains("REQ_CONTROL_ID"), "{err}");
        // Single pass: a default that looks like a placeholder is not interpolated again.
        let set = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Template {
                    template_id: "t".into(),
                },
            )],
            &[tpl("t", "{{A}}", &[("A", "{{NOW}}")])],
        )
        .unwrap();
        assert_eq!(
            reply_text(set.evaluate(&msg("ADT^A01", "1", ""))),
            "{{NOW}}"
        );
    }

    #[test]
    fn builtin_ack_template_works_as_a_rule_response() {
        let set = compile(
            vec![rule(
                "1",
                hl7("*"),
                None,
                Action::Template {
                    template_id: "builtin-hl7-ack-o21".into(),
                },
            )],
            &builtin_templates(),
        )
        .unwrap();
        let text = reply_text(set.evaluate(&msg("ADT^A01", "ID-7", "")));
        assert!(text.contains("<CR>MSA|AA|ID-7<CR>"), "{text}");
    }

    #[test]
    fn shape_validation_rejects_bad_rules() {
        let ok = rule("a", hl7("ADT^A01"), None, literal("<ACK>"));
        assert!(compile(vec![ok.clone()], &[]).is_ok());
        let mut cases: Vec<Rule> = Vec::new();
        let mut r = ok.clone();
        r.id = "bad id".into();
        cases.push(r);
        let mut r = ok.clone();
        r.name = "  ".into();
        cases.push(r);
        let mut r = ok.clone();
        r.delay_ms = MAX_DELAY_MS + 1;
        cases.push(r);
        let mut r = ok.clone();
        r.trigger = hl7("");
        cases.push(r);
        let mut r = ok.clone();
        r.trigger = hl7("A B");
        cases.push(r);
        let mut r = ok.clone();
        r.trigger = Trigger::AstmEnq;
        r.condition = Some(Condition {
            segment: "PID".into(),
            field: 1,
            operator: Operator::Exact,
            value: "x".into(),
        });
        cases.push(r);
        for (segment, field, value) in [
            ("pid", 1, "x"),
            ("PID", 0, "x"),
            ("PID", 1000, "x"),
            ("PID", 1, ""),
        ] {
            let mut r = ok.clone();
            r.condition = Some(Condition {
                segment: segment.into(),
                field,
                operator: Operator::Exact,
                value: value.into(),
            });
            cases.push(r);
        }
        let mut r = ok.clone();
        r.action = literal("  ");
        cases.push(r);
        let mut r = ok.clone();
        r.action = Action::Hl7Ack {
            message_type: "ACK".into(),
            code: "A".into(),
        };
        cases.push(r);
        let mut r = ok.clone();
        r.trigger = Trigger::AstmFrame;
        r.action = Action::Hl7Ack {
            message_type: "ACK".into(),
            code: "AA".into(),
        };
        cases.push(r);
        let mut r = ok.clone();
        r.action = Action::Template {
            template_id: String::new(),
        };
        cases.push(r);
        for (index, case) in cases.into_iter().enumerate() {
            assert!(compile(vec![case], &[]).is_err(), "case {index}");
        }
        assert!(compile(vec![ok.clone(), ok], &[]).is_err(), "duplicate ids");
    }

    #[test]
    fn serde_shape_is_strict() {
        let json = r#"{"enabled":true,"rules":[{"id":"r","name":"n","enabled":true,
            "trigger":{"type":"hl7","message_type":"QRY^A19"},
            "condition":{"segment":"QRD","field":8,"operator":"glob","value":"AAZ*"},
            "action":{"type":"hl7_ack","message_type":"ACK^A19","code":"AE"},"delay_ms":50}]}"#;
        let config: AutoReplyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.rules[0].delay_ms, 50);
        assert_eq!(
            serde_json::from_value::<AutoReplyConfig>(serde_json::to_value(&config).unwrap())
                .unwrap(),
            config
        );
        let extra = json.replace(r#""delay_ms":50"#, r#""delay_ms":50,"extra":1"#);
        assert!(serde_json::from_str::<AutoReplyConfig>(&extra).is_err());
        let unknown = json.replace("hl7_ack", "teleport");
        assert!(serde_json::from_str::<AutoReplyConfig>(&unknown).is_err());
        let negative = json.replace(r#""delay_ms":50"#, r#""delay_ms":-1"#);
        assert!(serde_json::from_str::<AutoReplyConfig>(&negative).is_err());
    }
}
