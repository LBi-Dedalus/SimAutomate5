use super::config::{FieldNumber, SegmentId, MAX_DELAY_MS, MAX_RULES};
use super::matching::glob_match;
use super::*;
use crate::config_store::{
    builtin_templates, AutobuildMode, Template, TemplateId, TemplateVariable,
};
use crate::protocol::frames::FrameEvent;
use crate::translate;

fn rule(
    id: &str,
    trigger: AutoReplyTrigger,
    condition: Option<AutoReplyCondition>,
    action: AutoReplyAction,
) -> RawAutoReplyRule {
    RawAutoReplyRule {
        id: id.to_string(),
        name: format!("Rule {id}"),
        enabled: true,
        trigger,
        condition,
        action,
        delay_ms: 0,
    }
}

fn hl7(pattern: &str) -> AutoReplyTrigger {
    AutoReplyTrigger::Hl7 {
        message_type: pattern.to_string(),
    }
}

fn literal(text: &str) -> AutoReplyAction {
    AutoReplyAction::Literal {
        text: text.to_string(),
    }
}

fn use_template(id: &str) -> AutoReplyAction {
    AutoReplyAction::Template {
        template_id: id.into(),
    }
}

fn ack(message_type: &str, code: &str) -> AutoReplyAction {
    AutoReplyAction::Hl7Ack {
        message_type: message_type.into(),
        code: code.into(),
    }
}

fn tpl(id: &str, payload: &str, defaults: &[(&str, &str)]) -> Template {
    Template {
        id: id.into(),
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
        autobuild: AutobuildMode::DISABLED,
    }
}

fn compile(
    rules: Vec<RawAutoReplyRule>,
    templates: &[Template],
) -> Result<AutoReplyRuleSet, String> {
    AutoReplyRuleSet::compile(
        &RawAutoReplyConfig {
            enabled: true,
            rules,
        }
        .parse()?,
        templates,
    )
}

fn msg(msh9: &str, id: &str, extra: &str) -> FrameEvent {
    FrameEvent::Hl7(format!("MSH|^~\\&|S|F|R|F|20260101||{msh9}|{id}|P|2.4\r{extra}").into_bytes())
}

fn reply_text(outcome: AutoReplyOutcome) -> String {
    match outcome {
        AutoReplyOutcome::Reply(reply) => reply
            .frames
            .iter()
            .map(|f| translate::ControlTokenDecoder::decode_all(f))
            .collect::<Vec<_>>()
            .join("|FRAME|"),
        other => panic!("expected a reply, got {other:?}"),
    }
}

fn none_when(
    id: &str,
    trigger: AutoReplyTrigger,
    condition: Option<AutoReplyCondition>,
) -> RawAutoReplyRule {
    rule(id, trigger, condition, AutoReplyAction::None)
}

fn cond(segment: &str, field: u32, operator: AutoReplyOperator, value: &str) -> AutoReplyCondition {
    AutoReplyCondition {
        segment: segment.into(),
        field: field.into(),
        operator,
        value: value.into(),
    }
}

fn qrd8(value: &str) -> Option<AutoReplyCondition> {
    Some(cond("QRD", 8, AutoReplyOperator::Glob, value))
}

fn literal_rule(id: &str) -> RawAutoReplyRule {
    rule(id, hl7("*"), None, literal("LATE"))
}

// ── Evaluation ─────────────────────────────────────────────────────────

#[test]
fn literal_and_template_text_is_utf8_on_the_wire() {
    let event = msg("QRY^A19", "X1", "");
    let set = compile(vec![rule("0", hl7("QRY*"), None, literal("é<STX>€"))], &[]).unwrap();
    match set.evaluate(&event) {
        AutoReplyOutcome::Reply(reply) => {
            assert_eq!(reply.frames, vec![vec![0xC3, 0xA9, 0x02, 0xE2, 0x82, 0xAC]]);
        }
        other => panic!("expected a reply, got {other:?}"),
    }
    let t = tpl("t", "é€", &[]);
    let set = compile(vec![rule("0", hl7("QRY*"), None, use_template("t"))], &[t]).unwrap();
    match set.evaluate(&event) {
        AutoReplyOutcome::Reply(reply) => {
            assert_eq!(reply.frames, vec![vec![0xC3, 0xA9, 0xE2, 0x82, 0xAC]]);
        }
        other => panic!("expected a reply, got {other:?}"),
    }
}

#[test]
fn no_auto_reply_suppresses_and_stops_lower_rules() {
    let set = compile(
        vec![
            none_when("quiet", hl7("QRY*"), qrd8("AAZ*")),
            rule("ack", hl7("*"), None, ack("ACK", "AA")),
            literal_rule("lit"),
        ],
        &[],
    )
    .unwrap();
    // Matching message: suppressed, with no frames, no failure and no later rule.
    match set.evaluate(&msg("QRY^A19", "X1", "QRD|1|1|1|1|1|1|1|AAZ9\r")) {
        AutoReplyOutcome::Suppressed { rule_name } => assert_eq!(rule_name, "Rule quiet"),
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
    assert!(matches!(
        set.evaluate(&no_id),
        AutoReplyOutcome::Suppressed { .. }
    ));
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
            none_when("enq", AutoReplyTrigger::AstmEnq, None),
            none_when("frame", AutoReplyTrigger::AstmFrame, None),
            rule("late", AutoReplyTrigger::AstmEnq, None, literal("<ACK>")),
        ],
        &[],
    )
    .unwrap();
    assert!(matches!(
        set.evaluate(&FrameEvent::Enq),
        AutoReplyOutcome::Suppressed { .. }
    ));
    assert!(matches!(
        set.evaluate(&FrameEvent::AstmFrame(b"1H|".to_vec())),
        AutoReplyOutcome::Suppressed { .. }
    ));
    // Master switch off: nothing evaluated at all.
    let off = AutoReplyRuleSet::compile(
        &RawAutoReplyConfig {
            enabled: false,
            rules: vec![none_when("enq", AutoReplyTrigger::AstmEnq, None)],
        }
        .parse()
        .unwrap(),
        &[],
    )
    .unwrap();
    assert!(matches!(
        off.evaluate(&FrameEvent::Enq),
        AutoReplyOutcome::NoMatch
    ));
}

#[test]
fn first_matching_rule_wins_and_otherwise_rule_comes_last() {
    let templates = vec![
        tpl("adr", "<VT>ADR {{REQ_CONTROL_ID}}<FS><CR>", &[]),
        tpl("nf", "NOTFOUND", &[]),
    ];
    let set = compile(
        vec![
            rule("1", hl7("QRY^A19"), qrd8("AAZ*"), use_template("adr")),
            rule("2", hl7("QRY^A19"), None, use_template("nf")),
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
        AutoReplyOutcome::NoMatch
    ));
}

#[test]
fn disabled_rules_and_disabled_master_never_reply() {
    let mut disabled = rule("1", hl7("*"), None, literal("<ACK>"));
    disabled.enabled = false;
    let fallback = rule("2", hl7("*"), None, literal("SECOND"));
    let set = compile(vec![disabled, fallback], &[]).unwrap();
    assert_eq!(reply_text(set.evaluate(&msg("ADT^A01", "1", ""))), "SECOND");

    let off = AutoReplyRuleSet::compile(
        &RawAutoReplyConfig {
            enabled: false,
            rules: vec![rule("1", hl7("*"), None, literal("X"))],
        }
        .parse()
        .unwrap(),
        &[],
    )
    .unwrap();
    assert!(matches!(
        off.evaluate(&msg("ADT^A01", "1", "")),
        AutoReplyOutcome::NoMatch
    ));
    assert!(matches!(
        AutoReplyRuleSet::disabled().evaluate(&FrameEvent::Enq),
        AutoReplyOutcome::NoMatch
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
        AutoReplyOutcome::NoMatch
    ));
    assert!(matches!(
        set.evaluate(&msg("ORU", "1", "")),
        AutoReplyOutcome::NoMatch
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
        AutoReplyOutcome::NoMatch
    ));
    assert!(matches!(
        generic.evaluate(&msg("ACK", "1", "")),
        AutoReplyOutcome::NoMatch
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
        b"MSH#*~\\&#S#F#R#F#2026##QRY*A19*QRY_A19#CTL#P#2.4\rQRD#1#2#3#4#5#6#7#AAZ*9\r".to_vec(),
    );
    let reply_for = |condition: AutoReplyCondition, text: &str| {
        let set = compile(
            vec![rule("0", hl7("QRY^A19"), Some(condition), literal(text))],
            &[],
        )
        .unwrap();
        set.evaluate(&event)
    };
    assert_eq!(
        reply_text(reply_for(
            cond("MSH", 1, AutoReplyOperator::Exact, "#"),
            "SEP"
        )),
        "SEP"
    );
    assert_eq!(
        reply_text(reply_for(
            cond("MSH", 2, AutoReplyOperator::Exact, "*~\\&"),
            "ENC"
        )),
        "ENC"
    );
    assert_eq!(
        reply_text(reply_for(
            cond("MSH", 10, AutoReplyOperator::Exact, "CTL"),
            "MSH10"
        )),
        "MSH10"
    );
    // Field values are compared raw: components are part of the value.
    assert_eq!(
        reply_text(reply_for(
            cond("QRD", 8, AutoReplyOperator::Exact, "AAZ*9"),
            "RAW"
        )),
        "RAW"
    );
    // Missing segment or field never matches.
    assert!(matches!(
        reply_for(cond("PID", 1, AutoReplyOperator::Exact, "x"), "NO"),
        AutoReplyOutcome::NoMatch
    ));
}

#[test]
fn astm_rules_match_frames_and_enq_only() {
    let set = compile(
        vec![
            rule("1", AutoReplyTrigger::AstmEnq, None, literal("<ACK>")),
            rule("2", AutoReplyTrigger::AstmFrame, None, literal("<ACK>")),
        ],
        &[],
    )
    .unwrap();
    assert!(matches!(
        set.evaluate(&FrameEvent::Enq),
        AutoReplyOutcome::Reply(_)
    ));
    assert!(matches!(
        set.evaluate(&FrameEvent::AstmFrame(b"\x021H\r\x0300\r\n".to_vec())),
        AutoReplyOutcome::Reply(_)
    ));
    assert!(matches!(
        set.evaluate(&msg("ADT^A01", "1", "")),
        AutoReplyOutcome::NoMatch
    ));
    assert!(matches!(
        set.evaluate(&FrameEvent::Malformed("x")),
        AutoReplyOutcome::NoMatch
    ));
}

#[test]
fn generated_ack_uses_rule_type_code_and_request_id() {
    let set = compile(
        vec![rule("1", hl7("ORU^R01"), None, ack("ACK^R01", "AE"))],
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
        vec![rule("1", hl7("*"), None, use_template("t"))],
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
fn autobuild_template_options_do_not_affect_auto_replies() {
    let plain = tpl("t", "<VT>A{{REQ_CONTROL_ID}}B<FS><CR>", &[]);
    let mut built = plain.clone();
    built.autobuild = AutobuildMode::Enabled { no_etb: true };
    let frames = |template: Template| {
        let set = compile(
            vec![rule("1", hl7("*"), None, use_template("t"))],
            &[template],
        )
        .unwrap();
        match set.evaluate(&msg("ADT^A01", "R1", "")) {
            AutoReplyOutcome::Reply(reply) => reply.frames,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(frames(plain), frames(built));
}

#[test]
fn request_data_is_never_translated_into_framing() {
    let templates = vec![tpl("t", "<VT>A{{REQ_CONTROL_ID}}B<FS><CR>", &[])];
    let set = compile(
        vec![rule("1", hl7("*"), None, use_template("t"))],
        &templates,
    )
    .unwrap();
    // Literal text that looks like control tokens stays literal wire bytes.
    let outcome = set.evaluate(&msg("ADT^A01", "x<FS><CR><VT>y", ""));
    match outcome {
        AutoReplyOutcome::Reply(reply) => {
            assert_eq!(reply.frames.len(), 1);
            assert_eq!(reply.frames[0], b"\x0bAx<FS><CR><VT>yB\x1c\r".to_vec());
        }
        other => panic!("{other:?}"),
    }
    // Raw control bytes in the request id are refused: nothing is sent.
    let bad = FrameEvent::Hl7(b"MSH|^~\\&|S|F|R|F|1||ADT^A01|a\x1cb|P|2.4\r".to_vec());
    assert!(matches!(
        set.evaluate(&bad),
        AutoReplyOutcome::Failed { .. }
    ));
    for id in [""] {
        let empty = msg("ADT^A01", id, "");
        assert!(matches!(
            set.evaluate(&empty),
            AutoReplyOutcome::Failed { .. }
        ));
    }
    // A failing rule does not fall through to a lower matching rule.
    let both = compile(
        vec![
            rule("1", hl7("*"), None, use_template("t")),
            rule("2", hl7("*"), None, literal("FALLBACK")),
        ],
        &templates,
    )
    .unwrap();
    assert!(matches!(
        both.evaluate(&msg("ADT^A01", "", "")),
        AutoReplyOutcome::Failed { .. }
    ));
    let ack_set = compile(vec![rule("1", hl7("*"), None, ack("ACK", "AA"))], &[]).unwrap();
    // With another field separator "|" is data, but it would break the generated ACK.
    let odd = FrameEvent::Hl7(b"MSH#^~\\&#S#F#R#F#1##ADT^A01#a|b#P#2.4\r".to_vec());
    assert!(matches!(
        ack_set.evaluate(&odd),
        AutoReplyOutcome::Failed { .. }
    ));
}

#[test]
fn request_control_id_is_echoed_byte_for_byte_even_when_not_utf8() {
    let templates = [tpl("t", "A{{REQ_CONTROL_ID}}B", &[])];
    let template_set = compile(
        vec![rule("1", hl7("*"), None, use_template("t"))],
        &templates,
    )
    .unwrap();
    let ack_set = compile(vec![rule("1", hl7("*"), None, ack("ACK", "AA"))], &[]).unwrap();
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
            AutoReplyOutcome::Reply(reply) => {
                let mut expected = b"A".to_vec();
                expected.extend_from_slice(&id);
                expected.push(b'B');
                assert_eq!(reply.frames, vec![expected], "template {id:?}");
            }
            other => panic!("{other:?}"),
        }
        match ack_set.evaluate(&event) {
            AutoReplyOutcome::Reply(reply) => {
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
        AutoReplyOutcome::Reply(reply) => assert_eq!(reply.frames, vec![b"AK\xe9YB".to_vec()]),
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
            AutoReplyOutcome::Failed { .. }
        ));
        assert!(matches!(
            ack_set.evaluate(&event),
            AutoReplyOutcome::Failed { .. }
        ));
    }
}

// ── Template compilation ───────────────────────────────────────────────

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
            vec![rule("1", hl7("*"), None, use_template("t"))],
            &[template],
        )
        .unwrap_err();
        assert!(err.contains("Rule 1") && err.contains(expected), "{err}");
    }
    let err = compile(vec![rule("1", hl7("*"), None, use_template("gone"))], &[]).unwrap_err();
    assert!(
        err.contains("Rule 1") && err.contains("does not exist"),
        "{err}"
    );
    // REQ_CONTROL_ID needs an HL7 request.
    let err = compile(
        vec![rule(
            "1",
            AutoReplyTrigger::AstmEnq,
            None,
            use_template("t"),
        )],
        &[tpl("t", "{{REQ_CONTROL_ID}}", &[])],
    )
    .unwrap_err();
    assert!(err.contains("REQ_CONTROL_ID"), "{err}");
    // Single pass: a default that looks like a placeholder is not interpolated again.
    let set = compile(
        vec![rule("1", hl7("*"), None, use_template("t"))],
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
            use_template("builtin-hl7-ack-o21"),
        )],
        &builtin_templates(),
    )
    .unwrap();
    let text = reply_text(set.evaluate(&msg("ADT^A01", "ID-7", "")));
    assert!(text.contains("<CR>MSA|AA|ID-7<CR>"), "{text}");
}

// ── Matching ───────────────────────────────────────────────────────────

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

// ── Validation and wire format ─────────────────────────────────────────

#[test]
fn shape_validation_rejects_bad_rules() {
    let ok = rule("a", hl7("ADT^A01"), None, literal("<ACK>"));
    assert!(compile(vec![ok.clone()], &[]).is_ok());
    let mut cases: Vec<RawAutoReplyRule> = Vec::new();
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
    r.trigger = AutoReplyTrigger::AstmEnq;
    r.condition = Some(cond("PID", 1, AutoReplyOperator::Exact, "x"));
    cases.push(r);
    for (segment, field, value) in [
        ("pid", 1, "x"),
        ("PID", 0, "x"),
        ("PID", 1000, "x"),
        ("PID", 1, ""),
    ] {
        let mut r = ok.clone();
        r.condition = Some(cond(segment, field, AutoReplyOperator::Exact, value));
        cases.push(r);
    }
    let mut r = ok.clone();
    r.action = literal("  ");
    cases.push(r);
    let mut r = ok.clone();
    r.action = ack("ACK", "A");
    cases.push(r);
    let mut r = ok.clone();
    r.trigger = AutoReplyTrigger::AstmFrame;
    r.action = ack("ACK", "AA");
    cases.push(r);
    let mut r = ok.clone();
    r.action = use_template("");
    cases.push(r);
    for (index, case) in cases.into_iter().enumerate() {
        assert!(compile(vec![case], &[]).is_err(), "case {index}");
    }
    assert!(compile(vec![ok.clone(), ok], &[]).is_err(), "duplicate ids");
}

#[test]
fn condition_validation_messages_are_unchanged() {
    let message = |segment: &str, field: u32| {
        let mut r = rule("a", hl7("ADT^A01"), None, literal("X"));
        r.condition = Some(cond(segment, field, AutoReplyOperator::Exact, "x"));
        compile(vec![r], &[]).unwrap_err()
    };
    assert_eq!(
        message("pid", 1),
        "rule \"Rule a\": the condition segment must be 3 upper-case letters/digits"
    );
    assert_eq!(
        message("1ID", 1),
        "rule \"Rule a\": the condition segment must be 3 upper-case letters/digits"
    );
    assert_eq!(
        message("PID", 0),
        "rule \"Rule a\": the condition field number must be between 1 and 999"
    );
    assert_eq!(
        message("PID", 1000),
        "rule \"Rule a\": the condition field number must be between 1 and 999"
    );
    let mut ok = rule("a", hl7("ADT^A01"), None, literal("X"));
    ok.condition = Some(cond("Z99", 999, AutoReplyOperator::Exact, "x"));
    assert!(compile(vec![ok], &[]).is_ok());
}

#[test]
fn max_rules_is_enforced() {
    let rules = (0..=MAX_RULES)
        .map(|i| rule(&format!("r{i}"), hl7("*"), None, literal("X")))
        .collect();
    assert!(compile(rules, &[]).is_err());
}

#[test]
fn no_auto_reply_serializes_as_type_none() {
    let json = serde_json::to_value(AutoReplyAction::None).unwrap();
    assert_eq!(json, serde_json::json!({ "type": "none" }));
    let back: AutoReplyAction = serde_json::from_value(json).unwrap();
    assert_eq!(back, AutoReplyAction::None);
}

#[test]
fn serde_shape_is_strict() {
    let json = r#"{"enabled":true,"rules":[{"id":"r","name":"n","enabled":true,
        "trigger":{"type":"hl7","message_type":"QRY^A19"},
        "condition":{"segment":"QRD","field":8,"operator":"glob","value":"AAZ*"},
        "action":{"type":"hl7_ack","message_type":"ACK^A19","code":"AE"},"delay_ms":50}]}"#;
    let config: AutoReplyConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.rules()[0].delay_ms(), 50);
    assert_eq!(
        serde_json::from_value::<AutoReplyConfig>(serde_json::to_value(&config).unwrap()).unwrap(),
        config
    );
    let extra = json.replace(r#""delay_ms":50"#, r#""delay_ms":50,"extra":1"#);
    assert!(serde_json::from_str::<AutoReplyConfig>(&extra).is_err());
    let unknown = json.replace("hl7_ack", "teleport");
    assert!(serde_json::from_str::<AutoReplyConfig>(&unknown).is_err());
    let negative = json.replace(r#""delay_ms":50"#, r#""delay_ms":-1"#);
    assert!(serde_json::from_str::<AutoReplyConfig>(&negative).is_err());
}

#[test]
fn newtypes_keep_the_wire_format_of_plain_strings_and_numbers() {
    let condition = cond("QRD", 8, AutoReplyOperator::Glob, "AAZ*");
    let json = serde_json::to_value(&condition).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"segment":"QRD","field":8,"operator":"glob","value":"AAZ*"})
    );
    assert_eq!(
        serde_json::from_value::<AutoReplyCondition>(json).unwrap(),
        condition
    );

    let action = use_template("builtin-x");
    let json = serde_json::to_value(&action).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"type":"template","template_id":"builtin-x"})
    );
    assert_eq!(
        serde_json::from_value::<AutoReplyAction>(json).unwrap(),
        action
    );

    assert_eq!(
        serde_json::to_string(&TemplateId::from("a")).unwrap(),
        "\"a\""
    );
    assert_eq!(
        serde_json::to_string(&SegmentId::from("PID")).unwrap(),
        "\"PID\""
    );
    assert_eq!(serde_json::to_string(&FieldNumber::from(5)).unwrap(), "5");
}

#[test]
fn invalid_condition_values_deserialize_raw_and_are_reported_by_parse() {
    // The raw wire format accepts any string / u32 so the error keeps naming the rule.
    let json = r#"{"enabled":true,"rules":[{"id":"r","name":"n","enabled":true,
        "trigger":{"type":"hl7","message_type":"*"},
        "condition":{"segment":"pid","field":0,"operator":"exact","value":"x"},
        "action":{"type":"literal","text":"X"},"delay_ms":0}]}"#;
    let config: RawAutoReplyConfig = serde_json::from_str(json).unwrap();
    let err = config.parse().unwrap_err();
    assert!(
        err.starts_with("rule \"n\": the condition segment"),
        "{err}"
    );
    // Deserializing the valid type applies the same rules.
    let err = serde_json::from_str::<AutoReplyConfig>(json).unwrap_err();
    assert!(
        err.to_string()
            .contains("rule \"n\": the condition segment"),
        "{err}"
    );
    // Out of range / negative numbers are still rejected by serde, as with the former u32.
    let negative = json.replace(r#""field":0"#, r#""field":-1"#);
    assert!(serde_json::from_str::<RawAutoReplyConfig>(&negative).is_err());
}
