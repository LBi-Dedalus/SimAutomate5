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
//!
//! Layout: `config` (persisted types + validation), `template` (placeholder compilation),
//! `engine` (compiled `AutoReplyRuleSet` + evaluation), `matching` (glob / trigger / condition matching)
//! and `hl7` (message parsing).

mod config;
mod engine;
mod hl7;
mod matching;
mod template;

#[cfg(test)]
mod tests;

pub use config::{
    AutoReplyAction, AutoReplyCondition, AutoReplyConfig, AutoReplyOperator, AutoReplyTrigger,
    RawAutoReplyConfig, RawAutoReplyRule,
};
pub use engine::{AutoReplyOutcome, AutoReplyRuleSet};
