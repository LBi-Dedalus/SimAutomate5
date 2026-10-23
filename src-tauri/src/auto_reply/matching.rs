//! Glob and trigger / condition matching.

use super::config::{AutoReplyCondition, AutoReplyOperator};
use crate::protocol::hl7::Hl7Message;

/// Fully anchored, case-sensitive match where `*` matches any run of characters (even empty).
pub(super) fn glob_match(pattern: &str, text: &str) -> bool {
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
pub(super) fn type_matches(pattern: &str, message_type: &str) -> bool {
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

pub(super) fn condition_matches(condition: &AutoReplyCondition, message: &Hl7Message) -> bool {
    match message.field(condition.segment.as_str(), condition.field.get()) {
        None => false,
        Some(value) => match condition.operator {
            AutoReplyOperator::Exact => value == condition.value,
            AutoReplyOperator::Glob => glob_match(&condition.value, &value),
        },
    }
}
