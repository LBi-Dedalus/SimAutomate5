//! Template placeholder scanning and compilation into frame parts.

use std::collections::BTreeSet;

use crate::config_store::{Template, TemplateVariable};
use crate::translate;

/// One fragment of a reply frame (one template line): either fixed bytes known at compile time,
/// or a value only known when the reply is built.
#[derive(Debug)]
pub(super) enum FramePart {
    /// Literal text and variable defaults, already encoded.
    Bytes(Vec<u8>),
    /// `{{NOW}}`.
    Now,
    /// `{{CONTROL_ID}}`.
    ControlId,
    /// `{{REQ_CONTROL_ID}}`: the control id of the request being answered.
    RequestControlId,
}

/// One entry of `frames` per template line; the reply frame is the concatenation of its parts.
#[derive(Debug)]
pub(super) struct CompiledTemplate {
    pub(super) frames: Vec<Vec<FramePart>>,
}

enum Token<'a> {
    Text(&'a str),
    Var(&'a str),
}

fn text_token(text: &str) -> Result<Token<'_>, String> {
    if text.contains("{{") || text.contains("}}") {
        return Err(
            "malformed placeholder: use {{NAME}} with letters, digits and underscores only".into(),
        );
    }
    Ok(Token::Text(text))
}

/// Same grammar as template-core.js: `{{NAME}}`, NAME = [A-Za-z_][A-Za-z0-9_]*. Stray `{{` or
/// `}}` left in the text are malformed placeholders.
fn scan_placeholders(source: &str) -> Result<Vec<Token<'_>>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'{') {
            let name_start = i + 2;
            let mut end = name_start;

            // Variable names cannot start with a number
            if end < bytes.len() && (bytes[end].is_ascii_alphabetic() || bytes[end] == b'_') {
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                if bytes.get(end) == Some(&b'}') && bytes.get(end + 1) == Some(&b'}') {
                    if text_start < i {
                        tokens.push(text_token(&source[text_start..i])?);
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
        tokens.push(text_token(&source[text_start..])?);
    }
    Ok(tokens)
}

impl CompiledTemplate {
    pub(super) fn compile(template: &Template, request_available: bool) -> Result<Self, String> {
        let mut frames = Vec::new();
        let mut unresolved: BTreeSet<&str> = BTreeSet::new();
        for line in template.payload.lines() {
            let mut parts = Vec::new();
            for token in scan_placeholders(line)? {
                match token {
                    Token::Text(text) => parts.push(FramePart::Bytes(
                        translate::ControlTokenEncoder::encode_all(text),
                    )),
                    Token::Var("NOW") => parts.push(FramePart::Now),
                    Token::Var("CONTROL_ID") => parts.push(FramePart::ControlId),
                    Token::Var("REQ_CONTROL_ID") => {
                        if !request_available {
                            return Err(
                                "{{REQ_CONTROL_ID}} is only available for HL7 message rules".into(),
                            );
                        }
                        parts.push(FramePart::RequestControlId);
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
                            unresolved.insert(name);
                        } else {
                            parts.push(FramePart::Bytes(
                                translate::ControlTokenEncoder::encode_all(default),
                            ));
                        }
                    }
                }
            }
            if !parts.is_empty() {
                frames.push(parts);
            }
        }
        if !unresolved.is_empty() {
            return Err(format!(
                "unresolved variable(s): {} (give them a default value in the template)",
                unresolved.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if frames.is_empty() {
            return Err("the template payload is empty".into());
        }
        Ok(Self { frames })
    }
}
