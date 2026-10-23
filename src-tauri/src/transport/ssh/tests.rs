use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use russh::client;
use russh::keys::{parse_public_key_base64, Algorithm, HashAlg, PublicKey};
use russh::ChannelOpenFailure;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

use super::forwarding::{classify_open_error, OpenErrorKind};
use super::session::{
    connect_transport, describe_connect_error, CancellableStream, OneShot, SshError, SshHandler,
};
use super::settings::{
    expand_path, load_host_settings, load_key, preferred_host_key_algorithms, resolve_key,
    resolve_user, verify_host_key, HostSettings, SshPaths, DEFAULT_KEY_FILES,
};
use crate::models::User;

#[test]
fn classify_open_error_retries_refused_target() {
    for reason in [
        ChannelOpenFailure::ConnectFailed,
        ChannelOpenFailure::ResourceShortage,
    ] {
        assert_eq!(
            classify_open_error(&russh::Error::ChannelOpenFailure(reason)),
            OpenErrorKind::Retry
        );
    }
}

#[test]
fn classify_open_error_is_fatal_otherwise() {
    let fatal = [
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited),
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::UnknownChannelType),
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::Other {
            code: 42,
            reason: "x".into(),
        }),
        russh::Error::Disconnect,
        russh::Error::SendError,
    ];
    for err in fatal {
        assert!(matches!(classify_open_error(&err), OpenErrorKind::Fatal(_)));
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "simautomate-ssh-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join(".ssh")).unwrap();
        Self(dir)
    }

    fn paths(&self) -> SshPaths {
        SshPaths {
            home: self.0.clone(),
        }
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

fn key(base64: &str) -> PublicKey {
    parse_public_key_base64(base64).unwrap()
}

fn user(name: &str) -> User {
    let req = crate::models::ConnectRequest::SshServerStartRequest {
        port: 1,
        ssh_host: "h".into(),
        ssh_port: 22,
        ssh_user: name.into(),
    };
    match crate::models::ConnectionSpec::try_from(req).unwrap() {
        crate::models::ConnectionSpec::SshServer { tunnel, .. } => tunnel.user.unwrap(),
        _ => unreachable!(),
    }
}

#[test]
fn user_resolution_prefers_explicit_then_config_then_local() {
    let configured = HostSettings {
        user: Some("cfg".into()),
        ..Default::default()
    };
    let none = HostSettings::default();
    assert_eq!(
        resolve_user(Some(&user("me")), &configured, Some("os".into())).unwrap(),
        "me"
    );
    assert_eq!(
        resolve_user(None, &configured, Some("os".into())).unwrap(),
        "cfg"
    );
    assert_eq!(resolve_user(None, &none, Some("os".into())).unwrap(), "os");
    assert!(resolve_user(None, &none, None).is_err());
    assert!(resolve_user(None, &none, Some("  ".into())).is_err());
}

#[test]
fn host_settings_come_from_the_matching_host_block() {
    let dir = TempDir::new();
    let config = dir.write(
        ".ssh/config",
        "Host other\n  User nope\n\nHost vm\n  HostName 10.0.0.5\n  User bob\n  IdentityFile ~\\.ssh\\vm_key\n",
    );
    let (settings, warning) = load_host_settings(&config, "vm");
    assert_eq!(warning, None);
    assert_eq!(settings.hostname.as_deref(), Some("10.0.0.5"));
    assert_eq!(settings.user.as_deref(), Some("bob"));
    assert_eq!(
        settings.identity_files,
        vec![PathBuf::from("~\\.ssh\\vm_key")]
    );

    let (unknown, _) = load_host_settings(&config, "elsewhere");
    assert_eq!(unknown, HostSettings::default());
}

#[test]
fn missing_or_unusable_ssh_config_falls_back_to_defaults() {
    let dir = TempDir::new();
    let (settings, warning) = load_host_settings(&dir.paths().config(), "vm");
    assert_eq!((settings, warning), (HostSettings::default(), None));

    let config = dir.write(".ssh/config", "Match all\n  User x\n");
    let (settings, warning) = load_host_settings(&config, "vm");
    assert_eq!(settings, HostSettings::default());
    assert!(warning.unwrap().contains("ignoring it"));
}

#[test]
fn paths_are_expanded_against_the_given_home() {
    let home = Path::new("C:\\Users\\me");
    assert_eq!(expand_path(Path::new("~"), home), home);
    assert_eq!(
        expand_path(Path::new("~\\.ssh\\k"), home),
        home.join(".ssh\\k")
    );
    assert_eq!(
        expand_path(Path::new("~/.ssh/k"), home),
        home.join(".ssh/k")
    );
    assert_eq!(
        expand_path(Path::new("%userprofile%\\k"), home),
        home.join("k")
    );
    assert_eq!(
        expand_path(Path::new("D:\\keys\\k"), home),
        PathBuf::from("D:\\keys\\k")
    );
}

#[test]
fn key_resolution_prefers_the_config_then_the_default_files() {
    let dir = TempDir::new();
    let paths = dir.paths();
    let none = HostSettings::default();
    let err = resolve_key(&none, &paths).unwrap_err();
    for name in DEFAULT_KEY_FILES {
        assert!(err.contains(name), "{err}");
    }

    let rsa = dir.write(".ssh/id_rsa", "x");
    assert_eq!(resolve_key(&none, &paths).unwrap(), rsa);
    let ed = dir.write(".ssh/id_ed25519", "x");
    assert_eq!(resolve_key(&none, &paths).unwrap(), ed);

    let custom = dir.write(".ssh/vm_key", "x");
    let configured = HostSettings {
        identity_files: vec![PathBuf::from("~/.ssh/vm_key")],
        ..Default::default()
    };
    // `~` in a hand built setting is expanded against the injected home.
    assert_eq!(resolve_key(&configured, &paths).unwrap(), custom);

    let missing = HostSettings {
        identity_files: vec![dir.0.join("absent")],
        ..Default::default()
    };
    // A configured file that does not exist falls back to the default ones.
    assert_eq!(resolve_key(&missing, &paths).unwrap(), ed);
    let err = resolve_key(&missing, &TempDir::new().paths()).unwrap_err();
    assert!(err.contains("absent"), "{err}");
}

#[test]
fn encrypted_and_missing_keys_have_clear_errors() {
    let dir = TempDir::new();
    let absent = dir.0.join("nokey");
    assert!(load_key(&absent).unwrap_err().contains("nokey"));

    let garbage = dir.write(".ssh/garbage", "not a key");
    assert!(load_key(&garbage).unwrap_err().contains("Cannot load"));
}

#[test]
fn host_key_check_follows_known_hosts() {
    let dir = TempDir::new();
    let known = dir.write(
        ".ssh/known_hosts",
        &format!("# comment\nvm ssh-ed25519 {KEY_A}\n[vm2]:2222 ssh-ed25519 {KEY_A}\n"),
    );
    assert!(verify_host_key(&known, "vm", "vm", 22, &key(KEY_A)).is_ok());
    assert!(verify_host_key(&known, "vm2", "vm2", 2222, &key(KEY_A)).is_ok());

    let changed = verify_host_key(&known, "vm", "vm", 22, &key(KEY_B)).unwrap_err();
    assert!(changed.contains("does not match line"), "{changed}");
    assert!(changed.contains("man-in-the-middle"), "{changed}");

    let unknown = verify_host_key(&known, "other", "alias", 22, &key(KEY_A)).unwrap_err();
    assert!(unknown.contains("Run `ssh alias`"), "{unknown}");
    let unknown = verify_host_key(&known, "vm2", "vm2", 22, &key(KEY_A)).unwrap_err();
    assert!(unknown.contains("not in"), "{unknown}");
    let unknown = verify_host_key(&known, "vm3", "vm3", 2200, &key(KEY_A)).unwrap_err();
    assert!(unknown.contains("ssh -p 2200 vm3"), "{unknown}");
}

#[test]
fn hashed_known_hosts_entries_are_matched() {
    // Entry produced by `ssh-keygen -H`, taken from the russh test suite (host: example.com).
    let dir = TempDir::new();
    let known = dir.write(
        ".ssh/known_hosts",
        "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF\n",
    );
    assert!(verify_host_key(&known, "example.com", "example.com", 22, &key(KEY_B)).is_ok());
    assert!(verify_host_key(&known, "elsewhere", "elsewhere", 22, &key(KEY_B)).is_err());
    assert!(verify_host_key(&known, "example.com", "example.com", 22, &key(KEY_A)).is_err());
}

#[test]
fn missing_known_hosts_means_unknown_host() {
    let dir = TempDir::new();
    let err = verify_host_key(&dir.paths().known_hosts(), "vm", "vm", 22, &key(KEY_A)).unwrap_err();
    assert!(err.contains("known_hosts"), "{err}");
}

#[test]
fn an_unreadable_known_hosts_is_an_error() {
    let dir = TempDir::new();
    // A directory in place of the file: russh cannot open it and sees no entry, so the host is
    // unknown. Either way the key is never accepted.
    std::fs::create_dir_all(dir.0.join(".ssh/known_hosts")).unwrap();
    let known = dir.paths().known_hosts();
    assert!(verify_host_key(&known, "vm", "vm", 22, &key(KEY_A)).is_err());
}

#[test]
fn recorded_host_key_algorithms_come_first() {
    let defaults = [
        Algorithm::Ed25519,
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha512),
        },
    ];
    let ecdsa = Algorithm::Ecdsa {
        curve: russh::keys::EcdsaCurve::NistP256,
    };
    assert_eq!(
        preferred_host_key_algorithms(std::slice::from_ref(&ecdsa), &defaults),
        vec![
            ecdsa,
            Algorithm::Ed25519,
            Algorithm::Rsa {
                hash: Some(HashAlg::Sha512)
            }
        ]
    );
    assert_eq!(
        preferred_host_key_algorithms(&[], &defaults),
        defaults.to_vec()
    );
    let rsa = preferred_host_key_algorithms(&[Algorithm::Rsa { hash: None }], &defaults);
    assert_eq!(
        rsa[0],
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha512)
        }
    );
    assert_eq!(rsa.iter().filter(|a| **a == Algorithm::Ed25519).count(), 1);
}

#[test]
fn one_shot_hands_off_once_whatever_the_receiver_does() {
    let (sender, mut receiver) = mpsc::channel(1);
    let mut gate = OneShot::new(sender);
    assert_eq!(gate.hand_off(1), Ok(()));
    // The slot is free again after the receive, yet the gate stays shut.
    assert_eq!(receiver.try_recv(), Ok(1));
    assert_eq!(gate.hand_off(2), Err(2));
    assert!(receiver.try_recv().is_err());

    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    let mut gate = OneShot::new(sender);
    assert_eq!(gate.hand_off(3), Err(3));
    assert_eq!(OneShot::<u8>::none().hand_off(4), Err(4));
}

#[test]
fn connect_errors_are_described() {
    assert_eq!(
        describe_connect_error("vm", 22, SshError::HostKey("bad key".into())),
        "bad key"
    );
    let text = describe_connect_error("vm", 22, SshError::Russh(russh::Error::Disconnect));
    assert!(text.contains("vm:22"), "{text}");
}

#[tokio::test]
async fn cancelling_a_stream_ends_reads_and_fails_writes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (local, _remote) = tokio::io::duplex(64);
    let (mut stream, cancel) = CancellableStream::new(local);
    let reader = tokio::spawn(async move {
        let mut buf = [0u8; 8];
        let read = stream.read(&mut buf).await;
        let write = stream.write_all(b"x").await;
        (read, write)
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!reader.is_finished(), "the read must be pending");
    cancel.cancel();
    let (read, write) = timeout(Duration::from_secs(2), reader)
        .await
        .expect("cancel wakes the pending read")
        .unwrap();
    assert_eq!(read.unwrap(), 0);
    assert_eq!(write.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn a_stalled_handshake_times_out_and_closes_the_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut peer, _) = listener.accept().await.unwrap();
        peer.write_all(b"SSH-2.0-test\r\n").await.unwrap();
        // Stalls after the banner; ends when the client closes the connection.
        let mut buf = [0u8; 4096];
        loop {
            match peer.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let dir = TempDir::new();
    let handler = SshHandler {
        host: "127.0.0.1".into(),
        alias: "127.0.0.1".into(),
        port,
        known_hosts: dir.paths().known_hosts(),
        forwarded: OneShot::none(),
    };
    let result = connect_transport(
        Arc::new(client::Config::default()),
        "127.0.0.1",
        port,
        handler,
        Duration::from_millis(300),
    )
    .await;
    match result {
        Err(message) => assert!(message.contains("Timed out"), "{message}"),
        Ok(_) => panic!("the handshake cannot succeed"),
    }
    timeout(Duration::from_secs(3), server)
        .await
        .expect("the connection must be closed after the timeout")
        .unwrap();
}
