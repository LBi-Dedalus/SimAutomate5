use tokio::io::AsyncWriteExt;

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persisted_rules_are_loaded_at_startup_and_invalid_ones_disable_the_feature() {
    let cfg = TestConfig::new();
    // Valid file: applied before any connection exists.
    cfg.store
        .save_auto_reply(&auto_reply_config(
            true,
            vec![rule("r", AutoReplyTrigger::AstmEnq, literal("<ACK>"), 0)],
        ))
        .await
        .unwrap();
    let (sink, state) = setup();
    init_auto_reply(&cfg.store, &state, &NullLogger).await;
    let mut peer = connected_client(&sink, &state, "s").await;
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 500).await, vec![0x06]);

    // Corrupt file, read by a fresh store (the first one serves its cached configuration):
    // disabled, and the file is left untouched.
    std::fs::write(
        &cfg.path,
        r#"{"auto_reply":{"enabled":true,"rules":[{"x":1}]}}"#,
    )
    .unwrap();
    let before = std::fs::read_to_string(&cfg.path).unwrap();
    let store2 = ConfigStore::new(cfg.path.clone());
    let (sink2, state2) = setup();
    init_auto_reply(&store2, &state2, &NullLogger).await;
    let mut peer2 = connected_client(&sink2, &state2, "s").await;
    peer2.write_all(&[0x05]).await.unwrap();
    expect_silence(&mut peer2).await;
    assert!(load_config(&store2).await.is_err());
    assert!(set_auto_reply_enabled(&store2, &state2, true)
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&cfg.path).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saving_unchanged_templates_keeps_default_auto_reply_running() {
    let cfg = TestConfig::new();
    let (sink, state) = setup();
    init_auto_reply(&cfg.store, &state, &NullLogger).await;
    let mut peer = connected_client(&sink, &state, "s").await;
    peer.write_all(&[0x05]).await.unwrap();
    let before = collect(&mut peer, 500).await;
    assert_eq!(before, vec![0x06], "default rules reply ACK to ENQ");

    let saved = save_templates(&cfg.store, &state, &config_store::builtin_templates())
        .await
        .unwrap();
    assert!(saved.auto_reply.enabled());
    let loaded = load_config(&cfg.store).await.unwrap();
    assert_eq!(saved, loaded);
    assert_eq!(saved, cfg.store.load().await.unwrap());
    peer.write_all(&[0x05]).await.unwrap();
    assert_eq!(collect(&mut peer, 500).await, before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revisions_increase_with_every_save_and_loads_report_the_latest() {
    let cfg = TestConfig::new();
    let (_sink, state) = setup();
    let templates = config_store::builtin_templates();
    let first = load_config(&cfg.store).await.unwrap();
    let again = load_config(&cfg.store).await.unwrap();
    assert_eq!(again.revision, first.revision);

    let a = save_templates(&cfg.store, &state, &templates)
        .await
        .unwrap();
    let b = set_auto_reply_enabled(&cfg.store, &state, false)
        .await
        .unwrap();
    let c = save_auto_reply(&cfg.store, &state, &b.auto_reply)
        .await
        .unwrap();
    assert!(first.revision < a.revision && a.revision < b.revision && b.revision < c.revision);
    let loaded = load_config(&cfg.store).await.unwrap();
    assert_eq!(loaded, c);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_saves_get_increasing_revisions_and_the_newest_one_wins() {
    let cfg = TestConfig::new();
    let (sink, state) = setup();
    // Each save toggles the switch; whatever the order in which they reach the manager, it
    // must end up running the newest revision.
    let save = |enabled: bool| set_auto_reply_enabled(&cfg.store, &state, enabled);
    let (r0, r1, r2, r3) = tokio::join!(save(true), save(false), save(true), save(false));
    let results = [r0.unwrap(), r1.unwrap(), r2.unwrap(), r3.unwrap()];

    let mut revisions: Vec<u64> = results.iter().map(|r| r.revision).collect();
    revisions.sort_unstable();
    assert_eq!(revisions, vec![1, 2, 3, 4]);
    let last = results.iter().max_by_key(|r| r.revision).unwrap();
    assert_eq!(load_config(&cfg.store).await.unwrap(), *last);

    let mut peer = connected_client(&sink, &state, "s").await;
    peer.write_all(&[0x05]).await.unwrap();
    if last.auto_reply.enabled() {
        assert_eq!(collect(&mut peer, 500).await, vec![0x06]);
    } else {
        expect_silence(&mut peer).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_manager_ignores_a_configuration_older_than_the_one_it_runs() {
    let cfg = TestConfig::new();
    let (sink, state) = setup();
    let old = load_config(&cfg.store).await.unwrap();
    assert!(old.auto_reply.enabled());
    assert!(state.apply_rules(&old).await);

    let newer = set_auto_reply_enabled(&cfg.store, &state, false)
        .await
        .unwrap();
    assert!(!state.apply_rules(&old).await, "stale revision");
    assert!(!state.apply_rules(&newer).await, "same revision");

    let mut peer = connected_client(&sink, &state, "s").await;
    peer.write_all(&[0x05]).await.unwrap();
    expect_silence(&mut peer).await;
}
