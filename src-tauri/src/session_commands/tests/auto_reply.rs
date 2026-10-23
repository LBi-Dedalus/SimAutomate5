use tokio::io::AsyncWriteExt;

use super::*;
use crate::auto_reply::{AutoReplyCondition, AutoReplyOperator};
use crate::config_store::{AutobuildMode, TemplateVariable};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_rules_apply_to_current_and_future_sessions_and_off_stops_them() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    let mut old = connected_client(&sink, &state, "old").await;
    old.write_all(&[0x05]).await.unwrap();
    expect_silence(&mut old).await;

    apply(
        &cfg,
        &state,
        true,
        vec![rule("r1", AutoReplyTrigger::AstmEnq, literal("<ACK>"), 0)],
    )
    .await;
    old.write_all(&[0x05]).await.unwrap();
    read_until(&mut old, &[0x06]).await;

    let mut fresh = connected_client(&sink, &state, "new").await;
    fresh.write_all(&[0x05]).await.unwrap();
    read_until(&mut fresh, &[0x06]).await;

    let port = free_port();
    connect(&state, "srv", 1, server(port)).await.unwrap();
    wait_for("listening", || {
        sink.has_status("srv", 1, ConnectionStatus::Listening)
    })
    .await;
    let mut srv_peer = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    srv_peer.write_all(&[0x05]).await.unwrap();
    read_until(&mut srv_peer, &[0x06]).await;

    // The master switch alone turns every session off, without touching the rules.
    set_auto_reply_enabled(&cfg.store, &state, false)
        .await
        .unwrap();
    old.write_all(&[0x05]).await.unwrap();
    fresh.write_all(&[0x05]).await.unwrap();
    expect_silence(&mut old).await;
    expect_silence(&mut fresh).await;
    let loaded = load_config(&cfg.store).await.unwrap().auto_reply.clone();
    assert!(!loaded.enabled() && loaded.rules().len() == 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_matching_rule_wins_with_templates_and_request_control_id() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    let templates = vec![
        Template {
            id: "adr".into(),
            name: "ADR".into(),
            description: String::new(),
            payload: "<VT>MSH|^~\\&|X|Y|R|F|{{NOW}}||ADR^A19|{{CONTROL_ID}}|P|2.4<CR>MSA|AA|{{REQ_CONTROL_ID}}<CR><FS><CR>".into(),
            variables: vec![TemplateVariable { name: "REQ_CONTROL_ID".into(), default: "STALE".into() }],
            autobuild: AutobuildMode::DISABLED,
        },
        Template {
            id: "nf".into(),
            name: "Not found".into(),
            description: String::new(),
            payload: "NOT-FOUND {{REQ_CONTROL_ID}}".into(),
            variables: vec![],
            autobuild: AutobuildMode::DISABLED,
        },
    ];
    // Start without the default rules: they reference built-in templates the test replaces.
    apply(&cfg, &state, false, vec![]).await;
    save_templates(&cfg.store, &state, &templates)
        .await
        .unwrap();
    let mut specific = rule("a", hl7_trigger("QRY^A19"), template_action("adr"), 0);
    specific.condition = Some(AutoReplyCondition {
        segment: "QRD".into(),
        field: 8.into(),
        operator: AutoReplyOperator::Glob,
        value: "AAZ*".into(),
    });
    apply(
        &cfg,
        &state,
        true,
        vec![
            specific,
            rule("b", hl7_trigger("QRY^A19"), template_action("nf"), 0),
        ],
    )
    .await;

    let mut peer = connected_client(&sink, &state, "s").await;
    peer.write_all(&hl7_frame("QRY^A19", "REQ-1", "QRD|1|2|3|4|5|6|7|AAZ42\r"))
        .await
        .unwrap();
    let first = text(&read_until(&mut peer, b"\x1c\r").await);
    assert!(
        first.contains("||ADR^A19|") && first.contains("MSA|AA|REQ-1\r"),
        "{first:?}"
    );
    assert!(!first.contains("STALE"));

    peer.write_all(&hl7_frame("QRY^A19", "REQ-2", "QRD|1|2|3|4|5|6|7|OTHER\r"))
        .await
        .unwrap();
    assert_eq!(
        text(&read_until(&mut peer, b"REQ-2").await),
        "NOT-FOUND REQ-2"
    );

    // Unmatched and disabled: nothing at all.
    peer.write_all(&hl7_frame("ADT^A01", "REQ-3", ""))
        .await
        .unwrap();
    expect_silence(&mut peer).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_auto_reply_rule_silences_only_its_matches_and_stops_lower_rules() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    let mut quiet = rule("quiet", hl7_trigger("QRY^A19"), AutoReplyAction::None, 0);
    quiet.condition = Some(AutoReplyCondition {
        segment: "QRD".into(),
        field: 8.into(),
        operator: AutoReplyOperator::Glob,
        value: "AAZ*".into(),
    });
    let general = rule(
        "ack",
        hl7_trigger("*"),
        AutoReplyAction::Hl7Ack {
            message_type: "ACK".into(),
            code: "AA".into(),
        },
        0,
    );
    apply(&cfg, &state, true, vec![quiet.clone(), general.clone()]).await;

    let mut peer = connected_client(&sink, &state, "s").await;
    let mut other = connected_client(&sink, &state, "o").await;
    peer.write_all(&hl7_frame("QRY^A19", "Q-1", "QRD|1|2|3|4|5|6|7|AAZ42\r"))
        .await
        .unwrap();
    expect_silence(&mut peer).await;
    assert!(system_errors(&sink, "s").is_empty());

    // Another request on the same connection still gets the general acknowledgement.
    peer.write_all(&hl7_frame("QRY^A19", "Q-2", "QRD|1|2|3|4|5|6|7|OTHER\r"))
        .await
        .unwrap();
    let ack = text(&read_until(&mut peer, b"\x1c\r").await);
    assert!(ack.contains("MSA|AA|Q-2\r"), "{ack:?}");
    // Another session and manual sending are not affected.
    other
        .write_all(&hl7_frame("ADT^A01", "A-1", ""))
        .await
        .unwrap();
    assert!(text(&read_until(&mut other, b"\x1c\r").await).contains("MSA|AA|A-1\r"));
    send(&state, "s", 1, &msg("MANUAL")).await.unwrap();
    read_until(&mut peer, b"MANUAL").await;

    // Disabling the rule makes the general rule answer again.
    quiet.enabled = false;
    apply(&cfg, &state, true, vec![quiet, general]).await;
    peer.write_all(&hl7_frame("QRY^A19", "Q-3", "QRD|1|2|3|4|5|6|7|AAZ42\r"))
        .await
        .unwrap();
    assert!(text(&read_until(&mut peer, b"\x1c\r").await).contains("MSA|AA|Q-3\r"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generated_ack_is_configured_in_the_rule() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![rule(
            "ack",
            hl7_trigger("ORU^R01"),
            AutoReplyAction::Hl7Ack {
                message_type: "ACK^R01".into(),
                code: "AE".into(),
            },
            0,
        )],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;
    peer.write_all(&hl7_frame("ORU^R01^ORU_R01", "ID-9", ""))
        .await
        .unwrap();
    let reply = text(&read_until(&mut peer, b"\x1c\r").await);
    assert!(
        reply.contains("||ACK^R01|") && reply.contains("\rMSA|AE|ID-9\r"),
        "{reply:?}"
    );
    // A generic catch-all never answers that acknowledgement back.
    apply(
        &cfg,
        &state,
        true,
        vec![rule("all", hl7_trigger("*"), literal("LOOP"), 0)],
    )
    .await;
    peer.write_all(&hl7_frame("ACK^R01", "ID-10", "MSA|AA|ID-9\r"))
        .await
        .unwrap();
    expect_silence(&mut peer).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn split_and_coalesced_frames_reply_once_per_complete_message() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![
            rule("h", hl7_trigger("*"), literal("HL7-REPLY"), 0),
            rule("a", AutoReplyTrigger::AstmFrame, literal("ASTM-REPLY"), 0),
        ],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;

    let frame = hl7_frame("ADT^A01", "C1", "PID|1\r");
    let (head, tail) = frame.split_at(10);
    peer.write_all(head).await.unwrap();
    expect_silence(&mut peer).await;
    let (body, end) = tail.split_at(tail.len() - 2);
    peer.write_all(body).await.unwrap();
    peer.write_all(&end[..1]).await.unwrap(); // FS without its CR
    expect_silence(&mut peer).await;
    peer.write_all(&end[1..]).await.unwrap();
    assert_eq!(text(&collect(&mut peer, 600).await), "HL7-REPLY");

    let mut two = frame.clone();
    two.extend_from_slice(&frame);
    peer.write_all(&two).await.unwrap();
    assert_eq!(text(&collect(&mut peer, 1200).await), "HL7-REPLYHL7-REPLY");

    // ASTM: nothing before the checksum and the CR LF are there.
    peer.write_all(&ASTM_FRAME[..ASTM_FRAME.len() - 3])
        .await
        .unwrap();
    expect_silence(&mut peer).await;
    peer.write_all(&ASTM_FRAME[ASTM_FRAME.len() - 3..])
        .await
        .unwrap();
    assert_eq!(text(&collect(&mut peer, 600).await), "ASTM-REPLY");
    // The UI still got the raw chunks (one event per read, not per frame).
    assert!(sink.received("s").len() >= 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enq_is_acknowledged_but_ack_nak_eot_are_never_triggers() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![
            rule("enq", AutoReplyTrigger::AstmEnq, literal("<ACK>"), 0),
            rule("frame", AutoReplyTrigger::AstmFrame, literal("<ACK>"), 0),
        ],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;
    for byte in [0x06u8, 0x15, 0x04] {
        peer.write_all(&[byte]).await.unwrap();
    }
    expect_silence(&mut peer).await;
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 600).await, vec![0x06]);
    peer.write_all(ASTM_FRAME).await.unwrap();
    assert_eq!(collect(&mut peer, 600).await, vec![0x06]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delays_are_honoured_without_blocking_other_replies() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![
            rule("slow", hl7_trigger("*"), literal("SLOW"), 900),
            rule("fast", AutoReplyTrigger::AstmEnq, literal("<ACK>"), 0),
            rule("mid", AutoReplyTrigger::AstmFrame, literal("MID"), 300),
        ],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;

    // A 300 ms delay: nothing after 200 ms, the reply within 1.5 s.
    let started = Instant::now();
    peer.write_all(ASTM_FRAME).await.unwrap();
    assert!(collect(&mut peer, 200).await.is_empty());
    let got = read_until(&mut peer, b"MID").await;
    assert_eq!(text(&got), "MID");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300) && elapsed < Duration::from_millis(1500),
        "{elapsed:?}"
    );

    // The immediate ACK is not held behind the delayed HL7 reply.
    peer.write_all(&hl7_frame("ADT^A01", "C1", ""))
        .await
        .unwrap();
    peer.write_all(&[0x05]).await.unwrap();
    let first = collect(&mut peer, 500).await;
    assert_eq!(first, vec![0x06]);
    assert_eq!(text(&read_until(&mut peer, b"SLOW").await), "SLOW");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_replies_never_reach_a_new_peer_or_survive_changes() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![rule(
            "slow",
            AutoReplyTrigger::AstmEnq,
            literal("<ACK>"),
            700,
        )],
    )
    .await;

    let mut old = connected_client(&sink, &state, "s").await;
    old.write_all(&[0x05]).await.unwrap();
    sleep(Duration::from_millis(100)).await;
    stop(&state, "s", StopMode::Disconnect).await.unwrap();
    let (listener, port) = peer().await;
    connect(&state, "s", 2, client(port)).await.unwrap();
    let (mut new_peer, _) = listener.accept().await.unwrap();
    wait_for("reconnected", || {
        sink.has_status("s", 2, ConnectionStatus::Connected)
    })
    .await;
    assert!(collect(&mut new_peer, 1200).await.is_empty());
    assert!(collect(&mut old, 100).await.is_empty());

    // A configuration change drops what was scheduled by the previous rules.
    new_peer.write_all(&[0x05]).await.unwrap();
    sleep(Duration::from_millis(100)).await;
    set_auto_reply_enabled(&cfg.store, &state, false)
        .await
        .unwrap();
    assert!(collect(&mut new_peer, 1200).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_reply_in_isolation() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![rule("enq", AutoReplyTrigger::AstmEnq, literal("<ACK>"), 0)],
    )
    .await;
    let mut a = connected_client(&sink, &state, "a").await;
    let mut b = connected_client(&sink, &state, "b").await;
    a.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut a, 500).await, vec![0x06]);
    assert!(collect(&mut b, 300).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coalesced_ack_releases_the_queue_and_the_rest_is_still_processed() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![rule("enq", AutoReplyTrigger::AstmEnq, literal("<ACK>"), 0)],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;
    send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
    read_until(&mut peer, &[0x05]).await;

    // One chunk: the ACK of our ENQ, then the peer's own ENQ.
    peer.write_all(&[0x06, 0x05]).await.unwrap();
    let got = read_until(&mut peer, b"next").await;
    assert!(got.contains(&0x06), "automatic ACK missing: {got:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ack_releases_the_queue_even_when_rules_are_disabled_or_coalesced_with_frames() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(&cfg, &state, false, vec![]).await;
    let mut peer = connected_client(&sink, &state, "s").await;
    for (chunk, label) in [
        (vec![0x06u8], "plain"),
        ([&[0x06u8][..], ASTM_FRAME].concat(), "ack+astm"),
        (
            [&[0x06u8, 0x06][..], &hl7_frame("ADT^A01", "1", "")].concat(),
            "ack+ack+mllp",
        ),
    ] {
        send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
        read_until(&mut peer, &[0x05]).await;
        peer.write_all(&chunk).await.unwrap();
        read_until(&mut peer, b"next").await;
        assert!(!label.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ack_bytes_inside_a_frame_do_not_release_the_queue() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(&cfg, &state, false, vec![]).await;
    let mut peer = connected_client(&sink, &state, "s").await;
    send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
    read_until(&mut peer, &[0x05]).await;
    peer.write_all(&hl7_frame("ADT^A01", "1", "NTE|\u{6}\r"))
        .await
        .unwrap();
    peer.write_all(b"\x021H|\x06\r\x030B\r\n").await.unwrap();
    assert!(collect(&mut peer, 600).await.is_empty());
    peer.write_all(&[0x06]).await.unwrap();
    read_until(&mut peer, b"next").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_pure_ack_bypasses_the_wait_for_our_own_ack() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    apply(
        &cfg,
        &state,
        true,
        vec![
            rule("enq", AutoReplyTrigger::AstmEnq, literal("<ACK>"), 0),
            rule("frame", AutoReplyTrigger::AstmFrame, literal("REPLY"), 0),
        ],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;
    send(&state, "s", 1, &msg("<ENQ>\nnext")).await.unwrap();
    read_until(&mut peer, &[0x05]).await;

    // We wait for the ACK of our ENQ; the peer's ENQ is still answered at once.
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 600).await, vec![0x06]);

    // Any other automatic frame waits, like the queued user line.
    peer.write_all(ASTM_FRAME).await.unwrap();
    assert!(collect(&mut peer, 600).await.is_empty());
    peer.write_all(&[0x06]).await.unwrap();
    let released = text(&read_until(&mut peer, b"next").await);
    assert!(
        released.find("REPLY").unwrap() < released.find("next").unwrap(),
        "{released:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_text_never_becomes_framing_and_failures_are_visible() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    let templates = vec![Template {
        id: "t".into(),
        name: "T".into(),
        description: String::new(),
        payload: "<VT>A{{REQ_CONTROL_ID}}B<FS><CR>".into(),
        variables: vec![],
        autobuild: AutobuildMode::DISABLED,
    }];
    // Start without the default rules: they reference built-in templates the test replaces.
    apply(&cfg, &state, false, vec![]).await;
    save_templates(&cfg.store, &state, &templates)
        .await
        .unwrap();
    apply(
        &cfg,
        &state,
        true,
        vec![rule("t", hl7_trigger("*"), template_action("t"), 0)],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;

    peer.write_all(&hl7_frame("ADT^A01", "x<FS><CR><VT>y", ""))
        .await
        .unwrap();
    let reply = collect(&mut peer, 700).await;
    assert_eq!(reply, b"\x0bAx<FS><CR><VT>yB\x1c\r".to_vec());

    // A raw control byte in MSH-10, then an empty one: nothing is sent, the error is visible.
    let mut bad = hl7_frame("ADT^A01", "a", "");
    let at = bad.iter().position(|b| *b == b'a').unwrap();
    bad[at] = 0x1d;
    peer.write_all(&bad).await.unwrap();
    peer.write_all(&hl7_frame("ADT^A01", "", "")).await.unwrap();
    assert!(collect(&mut peer, 700).await.is_empty());
    wait_for("failure reported", || system_errors(&sink, "s").len() >= 2).await;
    assert!(system_errors(&sink, "s")[0].contains("Rule t"));

    // A MSH-10 that is not UTF-8 is echoed byte for byte (the peer's own bytes, not U+FFFD).
    let mut latin = hl7_frame("ADT^A01", "ID-a", "");
    let at = latin.iter().position(|b| *b == b'a').unwrap();
    latin[at] = 0xe9;
    peer.write_all(&latin).await.unwrap();
    let reply = collect(&mut peer, 700).await;
    assert_eq!(reply, b"\x0bAID-\xe9B\x1c\r".to_vec());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn template_changes_refresh_running_sessions_and_refusals_keep_the_engine() {
    let (sink, state) = setup();
    let cfg = TestConfig::new();
    let make = |payload: &str| Template {
        id: "t".into(),
        name: "T".into(),
        description: String::new(),
        payload: payload.into(),
        variables: vec![],
        autobuild: AutobuildMode::DISABLED,
    };
    // Start without the default rules: they reference built-in templates the test replaces.
    apply(&cfg, &state, false, vec![]).await;
    save_templates(&cfg.store, &state, &[make("<ACK>")])
        .await
        .unwrap();
    apply(
        &cfg,
        &state,
        true,
        vec![rule(
            "r",
            AutoReplyTrigger::AstmEnq,
            template_action("t"),
            0,
        )],
    )
    .await;
    let mut peer = connected_client(&sink, &state, "s").await;
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 500).await, vec![0x06]);

    // A valid edit is live for the running session.
    save_templates(&cfg.store, &state, &[make("<NAK>")])
        .await
        .unwrap();
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 500).await, vec![0x15]);

    // Breaking or deleting the referenced template is refused; the file and the engine stay.
    let before = std::fs::read_to_string(&cfg.path).unwrap();
    let err = save_templates(&cfg.store, &state, &[make("{{OOPS}}")])
        .await
        .unwrap_err();
    assert!(err.contains("Rule r"), "{err}");
    let err = save_templates(&cfg.store, &state, &[]).await.unwrap_err();
    assert!(err.contains("Rule r"), "{err}");
    assert_eq!(std::fs::read_to_string(&cfg.path).unwrap(), before);
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 500).await, vec![0x15]);

    // A rejected rule save keeps the previous engine as well.
    let mut broken = rule(
        "r",
        AutoReplyTrigger::AstmEnq,
        template_action("missing"),
        0,
    );
    broken.name = "Broken".into();
    assert!(
        save_auto_reply(&cfg.store, &state, &auto_reply_config(true, vec![broken]))
            .await
            .is_err()
    );
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 500).await, vec![0x15]);
}
