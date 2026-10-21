//! Tunnelling the simulator socket through SSH: `-L` (`client_task`, `direct-tcpip`) and `-R`
//! (`server_task`, `tcpip-forward` on the loopback of the SSH host).
//!
//! The SSH connection and authentication are attempted once; host keys are only ever read from
//! the user's `known_hosts` (never written); authentication uses an unencrypted private key file.

use std::borrow::Cow;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use russh::client::{self, Handle, Msg};
use russh::keys::known_hosts::{check_known_hosts_path, known_host_keys_path};
use russh::keys::{
    Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate,
};
use russh::{Channel, ChannelOpenFailure, Disconnect, Preferred};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Duration};

use super::streams::{serve, ConnectionContext};
use crate::emitter::Emitter;
use crate::models::{ConnectionStatus, Host, LogLevel, Port, SshTunnel, User};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_MAX: usize = 3;
const CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(1);
const CHANNEL_RETRY_DELAY: Duration = Duration::from_secs(1);
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_KEY_FILES: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];
/// Loopback of the SSH host: the server mode never opens a port to the network.
const REMOTE_BIND_ADDRESS: &str = "127.0.0.1";

#[derive(Debug)]
enum SshError {
    Russh(russh::Error),
    /// The host key was refused; the message tells the user what to do.
    HostKey(String),
}

impl From<russh::Error> for SshError {
    fn from(err: russh::Error) -> Self {
        Self::Russh(err)
    }
}

/// Where the user's SSH files live; injectable so that the tests never touch the real home.
#[derive(Debug, Clone)]
struct SshPaths {
    home: PathBuf,
}

impl SshPaths {
    fn from_env() -> Result<Self, String> {
        ["USERPROFILE", "HOME"]
            .iter()
            .filter_map(std::env::var_os)
            .find(|value| !value.is_empty())
            .map(|home| Self { home: home.into() })
            .ok_or_else(|| "Cannot find the home directory (USERPROFILE / HOME)".to_string())
    }

    fn ssh_dir(&self) -> PathBuf {
        self.home.join(".ssh")
    }

    fn config(&self) -> PathBuf {
        self.ssh_dir().join("config")
    }

    fn known_hosts(&self) -> PathBuf {
        self.ssh_dir().join("known_hosts")
    }
}

/// What the matching `Host` blocks of `~/.ssh/config` say about the SSH host.
#[derive(Debug, Default, PartialEq, Eq)]
struct HostSettings {
    hostname: Option<String>,
    user: Option<String>,
    identity_files: Vec<PathBuf>,
}

/// The settings of `alias`, plus a warning when the file exists but cannot be used (the
/// connection then goes on with the defaults). A missing file is not a problem.
fn load_host_settings(config_path: &Path, alias: &str) -> (HostSettings, Option<String>) {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (HostSettings::default(), None)
        }
        Err(err) => {
            let warning = format!("Cannot read {}: {err}; ignoring it", config_path.display());
            return (HostSettings::default(), Some(warning));
        }
    };
    match russh_config::parse(&contents, alias) {
        Ok(config) => {
            let host_config = config.host_config;
            let settings = HostSettings {
                hostname: host_config.hostname,
                user: host_config.user,
                identity_files: host_config.identity_file.unwrap_or_default(),
            };
            (settings, None)
        }
        Err(err) => {
            let warning = format!("Cannot parse {}: {err}; ignoring it", config_path.display());
            (HostSettings::default(), Some(warning))
        }
    }
}

/// Expands a leading `~` or `%USERPROFILE%` (the SSH config crate only handles `~/`).
fn expand_path(path: &Path, home: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    let rest = if raw == "~" {
        Some("")
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        Some(rest)
    } else {
        let prefix = "%USERPROFILE%";
        match raw.get(..prefix.len()) {
            Some(head) if head.eq_ignore_ascii_case(prefix) => {
                Some(raw[prefix.len()..].trim_start_matches(['/', '\\']))
            }
            _ => None,
        }
    };
    match rest {
        Some("") => home.to_path_buf(),
        Some(rest) => home.join(rest),
        None => path.to_path_buf(),
    }
}

/// The first existing private key: the `IdentityFile`s of the SSH config, then the default
/// files. The error names every file that was tried.
fn resolve_key(settings: &HostSettings, paths: &SshPaths) -> Result<PathBuf, String> {
    let candidates: Vec<PathBuf> = settings
        .identity_files
        .iter()
        .map(|file| expand_path(file, &paths.home))
        .chain(
            DEFAULT_KEY_FILES
                .iter()
                .map(|name| paths.ssh_dir().join(name)),
        )
        .collect();
    if let Some(found) = candidates.iter().find(|candidate| candidate.is_file()) {
        return Ok(found.clone());
    }
    let tried: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect();
    Err(format!(
        "No private key found for the SSH connection; tried: {}",
        tried.join(", ")
    ))
}

/// The explicit user, else the `User` of the SSH config, else the local user name.
fn resolve_user(
    explicit: Option<&User>,
    settings: &HostSettings,
    local_user: Option<String>,
) -> Result<String, String> {
    explicit
        .map(|user| user.as_str().to_string())
        .or_else(|| settings.user.clone().filter(|user| !user.trim().is_empty()))
        .or_else(|| local_user.filter(|user| !user.trim().is_empty()))
        .ok_or_else(|| {
            "Cannot determine the SSH user: fill the SSH user field, or set User in ~/.ssh/config"
                .to_string()
        })
}

fn local_user_from_env() -> Option<String> {
    ["USERNAME", "USER"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
}

fn load_key(path: &Path) -> Result<PrivateKey, String> {
    russh::keys::load_secret_key(path, None).map_err(|err| match err {
        russh::keys::Error::KeyIsEncrypted => format!(
            "The private key {} requires a passphrase, passphrases are not supported",
            path.display()
        ),
        err => format!("Cannot load the private key {}: {err}", path.display()),
    })
}

/// `ssh` command that makes the user trust the host key.
fn trust_hint(alias: &str, port: u16) -> String {
    if port == 22 {
        format!("ssh {alias}")
    } else {
        format!("ssh -p {port} {alias}")
    }
}

/// Accepts the key only if the user's known_hosts already lists it for this host, using russh's
/// own (simpler than OpenSSH's) check; every other outcome is an error. `host` is the name
/// actually connected to, `alias` what the user typed (used in the hint).
fn verify_host_key(
    known_hosts: &Path,
    host: &str,
    alias: &str,
    port: u16,
    key: &PublicKey,
) -> Result<(), String> {
    match check_known_hosts_path(host, port, key, known_hosts) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "The host key of {host}:{port} is not in {}. Run `{}` once to check and trust it, then retry",
            known_hosts.display(),
            trust_hint(alias, port)
        )),
        Err(russh::keys::Error::KeyChanged { line }) => Err(format!(
            "WARNING: the host key of {host}:{port} does not match line {line} of {}. \
             This can be a man-in-the-middle attack, or the server was reinstalled. \
             If you trust it, remove that line and run `{}` once",
            known_hosts.display(),
            trust_hint(alias, port)
        )),
        Err(err) => Err(format!(
            "Cannot check the host key of {host}:{port} in {}: {err}",
            known_hosts.display()
        )),
    }
}

/// Host key algorithms, those already recorded in known_hosts first (like OpenSSH), so that the
/// server presents a key that can be checked.
fn preferred_host_key_algorithms(recorded: &[Algorithm], defaults: &[Algorithm]) -> Vec<Algorithm> {
    let mut preferred: Vec<Algorithm> = Vec::new();
    let mut push = |algorithm: Algorithm| {
        if !preferred.contains(&algorithm) {
            preferred.push(algorithm);
        }
    };
    for algorithm in recorded {
        match algorithm {
            // The recorded RSA key is valid for every signature hash.
            Algorithm::Rsa { .. } => {
                push(Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512),
                });
                push(Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256),
                });
                push(Algorithm::Rsa { hash: None });
            }
            other => push(other.clone()),
        }
    }
    for algorithm in defaults {
        push(algorithm.clone());
    }
    preferred
}

fn describe_connect_error(host: &str, port: u16, err: SshError) -> String {
    match err {
        SshError::HostKey(message) => message,
        SshError::Russh(err) => format!("SSH connection to {host}:{port} failed: {err}"),
    }
}

struct SshHandler {
    /// Name connected to (after the `Hostname` of the SSH config).
    host: String,
    alias: String,
    port: u16,
    known_hosts: PathBuf,
    /// Server mode: receives the first channel opened by the SSH host for the forwarded port.
    forwarded: OneShot<Channel<Msg>>,
}

impl client::Handler for SshHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        verify_host_key(
            &self.known_hosts,
            &self.host,
            &self.alias,
            self.port,
            &server_public_key.public_key(),
        )
        .map(|()| true)
        .map_err(SshError::HostKey)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        // Only one client is served: the gate hands off the first channel and refuses all later
        // ones, whatever the state of the receiver.
        match self.forwarded.hand_off(channel) {
            Ok(()) => reply.accept().await,
            Err(_refused) => {
                // Never confirmed: nothing is established, the SSH host just sees the refusal.
                reply
                    .reject(ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
            }
        }
        Ok(())
    }
}

/// Hands off at most one item, ever: the sender is taken on the first attempt, so later items are
/// refused even while the receiver is still alive.
struct OneShot<T>(Option<mpsc::Sender<T>>);

impl<T> OneShot<T> {
    fn new(sender: mpsc::Sender<T>) -> Self {
        Self(Some(sender))
    }

    fn none() -> Self {
        Self(None)
    }

    fn hand_off(&mut self, item: T) -> Result<(), T> {
        match self.0.take() {
            Some(sender) => sender.try_send(item).map_err(|err| err.into_inner()),
            None => Err(item),
        }
    }
}

/// Explicitly closes the channels nobody will serve (a bare russh channel does not send a close
/// when dropped).
async fn close_unserved(mut receiver: mpsc::Receiver<Channel<Msg>>) {
    receiver.close();
    while let Ok(channel) = receiver.try_recv() {
        let _ = channel.close().await;
    }
}

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

/// Cuts a `CancellableStream`: pending and later reads see EOF, writes fail.
#[derive(Clone, Default)]
struct CancelHandle(Arc<CancelState>);

impl CancelHandle {
    fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        let wakers = std::mem::take(&mut *self.0.wakers.lock().unwrap_or_else(|e| e.into_inner()));
        for waker in wakers {
            waker.wake();
        }
    }

    fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Must be called before checking `is_cancelled`, so that a cancel cannot be missed.
    fn register(&self, waker: &Waker) {
        let mut wakers = self.0.wakers.lock().unwrap_or_else(|e| e.into_inner());
        if !wakers.iter().any(|w| w.will_wake(waker)) {
            wakers.push(waker.clone());
        }
    }
}

/// Cancels the transport when dropped, unless disarmed: the russh session task owns the stream
/// from the start of the key exchange and would otherwise outlive a cancelled handshake.
struct CancelOnDrop(Option<CancelHandle>);

impl CancelOnDrop {
    fn disarm(mut self) -> CancelHandle {
        self.0.take().expect("disarmed once")
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.cancel();
        }
    }
}

struct CancellableStream<S> {
    inner: S,
    cancel: CancelHandle,
}

impl<S> CancellableStream<S> {
    fn new(inner: S) -> (Self, CancelHandle) {
        let cancel = CancelHandle::default();
        (
            Self {
                inner,
                cancel: cancel.clone(),
            },
            cancel,
        )
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CancellableStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.cancel.register(cx.waker());
        if self.cancel.is_cancelled() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CancellableStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.cancel.register(cx.waker());
        if self.cancel.is_cancelled() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.cancel.register(cx.waker());
        if self.cancel.is_cancelled() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.cancel.is_cancelled() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// TCP connect + SSH handshake within `budget`. The transport is owned here so that any early
/// exit (timeout, error, the future being dropped) cuts it and ends the russh session task.
async fn connect_transport(
    config: Arc<client::Config>,
    host: &str,
    port: u16,
    handler: SshHandler,
    budget: Duration,
) -> Result<(Handle<SshHandler>, CancelHandle), String> {
    let attempt = async {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|err| SshError::Russh(err.into()))?;
        if config.nodelay {
            let _ = tcp.set_nodelay(true);
        }
        let (stream, cancel) = CancellableStream::new(tcp);
        let guard = CancelOnDrop(Some(cancel));
        let handle = client::connect_stream(config, stream, handler).await?;
        Ok::<_, SshError>((handle, guard.disarm()))
    };
    timeout(budget, attempt)
        .await
        .map_err(|_| {
            format!(
                "Timed out connecting to {host}:{port} after {} s",
                budget.as_secs()
            )
        })?
        .map_err(|err| describe_connect_error(host, port, err))
}

/// An SSH connection that is always disconnected when it goes away: explicitly with `close`
/// (awaited, so a stop really waits for it), or from `Drop` when the task is aborted. The
/// transport is cut afterwards, so the russh session task ends even if the disconnect fails.
struct SshSession {
    handle: Option<Handle<SshHandler>>,
    transport: CancelHandle,
}

impl SshSession {
    fn new(handle: Handle<SshHandler>, transport: CancelHandle) -> Self {
        Self {
            handle: Some(handle),
            transport,
        }
    }

    async fn close(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = timeout(DISCONNECT_TIMEOUT, disconnect(&handle)).await;
        }
        self.transport.cancel();
    }
}

async fn disconnect(handle: &Handle<SshHandler>) {
    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
}

impl Deref for SshSession {
    type Target = Handle<SshHandler>;

    fn deref(&self) -> &Self::Target {
        self.handle
            .as_ref()
            .expect("the handle is only taken by close and drop")
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        let transport = self.transport.clone();
        match (self.handle.take(), tokio::runtime::Handle::try_current()) {
            (Some(handle), Ok(runtime)) => {
                runtime.spawn(async move {
                    let _ = timeout(DISCONNECT_TIMEOUT, disconnect(&handle)).await;
                    transport.cancel();
                });
            }
            _ => transport.cancel(),
        }
    }
}

/// Connects and authenticates, once and without retry (every failure here is fatal).
async fn open_session(
    emitter: &Emitter,
    tunnel: &SshTunnel,
    forwarded: OneShot<Channel<Msg>>,
) -> Result<SshSession, String> {
    let paths = SshPaths::from_env()?;
    let alias = tunnel.host.as_str();
    let port = tunnel.port.get();

    let (settings, warning) = load_host_settings(&paths.config(), alias);
    if let Some(warning) = warning {
        emitter.warn(file!(), line!(), warning);
    }
    let host = settings
        .hostname
        .clone()
        .unwrap_or_else(|| alias.to_string());
    let user = resolve_user(tunnel.user.as_ref(), &settings, local_user_from_env())?;
    let key_path = resolve_key(&settings, &paths)?;
    emitter.info(
        file!(),
        line!(),
        format!(
            "SSH: connecting to {host}:{port} as {user} with the key {}",
            key_path.display()
        ),
    );
    let key = Arc::new(load_key(&key_path)?);

    let known_hosts = paths.known_hosts();
    let recorded: Vec<Algorithm> = known_host_keys_path(&host, port, &known_hosts)
        .map(|keys| keys.iter().map(|(_, key)| key.algorithm()).collect())
        .unwrap_or_default();
    let config = client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: KEEPALIVE_MAX,
        inactivity_timeout: None,
        preferred: Preferred {
            key: Cow::Owned(preferred_host_key_algorithms(
                &recorded,
                &Preferred::DEFAULT.key,
            )),
            ..Preferred::DEFAULT
        },
        ..Default::default()
    };
    let handler = SshHandler {
        host: host.clone(),
        alias: alias.to_string(),
        port,
        known_hosts,
        forwarded,
    };

    let (connected, transport) =
        connect_transport(Arc::new(config), &host, port, handler, CONNECT_TIMEOUT).await?;
    let mut session = SshSession::new(connected, transport);
    emitter.info(
        file!(),
        line!(),
        format!("SSH: host key of {host}:{port} verified"),
    );

    let authenticated = timeout(CONNECT_TIMEOUT, async {
        let handle = session
            .handle
            .as_mut()
            .expect("the session was just created");
        let hash_alg = handle.best_supported_rsa_hash().await?.flatten();
        handle
            .authenticate_publickey(user.clone(), PrivateKeyWithHashAlg::new(key, hash_alg))
            .await
    })
    .await
    .map_err(|_| format!("Timed out authenticating as {user} on {host}:{port}"))?
    .map_err(|err| format!("SSH authentication as {user} on {host}:{port} failed: {err}"))?;
    if !authenticated.success() {
        return Err(format!(
            "SSH authentication refused for {user} on {host}:{port} with the key {}: \
             check the user name and that the public key is in ~/.ssh/authorized_keys on the host",
            key_path.display()
        ));
    }
    emitter.info(
        file!(),
        line!(),
        format!("SSH: authenticated as {user} on {host}:{port}"),
    );
    Ok(session)
}

/// Opens the `direct-tcpip` channel. The request is sent once and never abandoned (dropping the
/// future would not cancel it, and a late confirmation would leave an unserved channel): the same
/// future is polled until it resolves, with a warning every second while the target is slow.
/// An explicit refusal, or the death of the SSH session, is fatal.
async fn open_forward_channel(
    emitter: &Emitter,
    session: &SshSession,
    host: &Host,
    port: Port,
) -> Result<Channel<Msg>, ()> {
    let target = format!("{}:{}", host.as_str(), port.get());
    let mut attempt = 1;
    loop {
        emitter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("ssh channel opening attempt={attempt} address={target}"),
        );
        // One pending open at a time: a new one is only sent once the previous one resolved.
        let open = session.channel_open_direct_tcpip(
            host.as_str().to_string(),
            u32::from(port.get()),
            "127.0.0.1",
            0,
        );
        tokio::pin!(open);

        let mut waits = 1;
        let result = loop {
            match timeout(CHANNEL_OPEN_TIMEOUT, &mut open).await {
                Ok(result) => break result,
                Err(_) => {
                    emitter.warn(
                        file!(),
                        line!(),
                        format!(
                            "Connection to {target} through SSH timed out, still waiting for \
                             the target to accept: attempt={attempt} wait={waits}"
                        ),
                    );
                    waits += 1;
                }
            }
        };

        match result {
            Ok(channel) => {
                emitter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("ssh channel succeeded attempt={attempt} address={target}"),
                );
                return Ok(channel);
            }
            Err(err) => match classify_open_error(&err) {
                OpenErrorKind::Retry => {
                    emitter.warn(
                        file!(),
                        line!(),
                        format!(
                            "Target {target} not reachable from the SSH host ({err}), \
                             retrying in 1s: attempt={attempt}"
                        ),
                    );
                }
                OpenErrorKind::Fatal(message) => {
                    emitter.error(
                        file!(),
                        line!(),
                        format!("Connection to {target} through SSH failed: {message}"),
                    );
                    return Err(());
                }
            },
        }

        // Cancelled by the caller's shutdown select, like the rest of the connect phase.
        sleep(CHANNEL_RETRY_DELAY).await;
        attempt += 1;
    }
}

/// What to do after a failed `direct-tcpip` open.
#[derive(Debug, PartialEq, Eq)]
enum OpenErrorKind {
    /// The target does not accept (yet): open a new channel after a delay.
    Retry,
    /// Retrying is pointless or the session is broken: end with this message.
    Fatal(String),
}

fn classify_open_error(err: &russh::Error) -> OpenErrorKind {
    match err {
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)
        | russh::Error::ChannelOpenFailure(ChannelOpenFailure::ResourceShortage) => {
            OpenErrorKind::Retry
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
            OpenErrorKind::Fatal(
                "the SSH server forbids TCP forwarding (administratively prohibited): \
                 check AllowTcpForwarding / PermitOpen in sshd_config"
                    .to_string(),
            )
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::UnknownChannelType) => {
            OpenErrorKind::Fatal(
                "the SSH server does not support direct-tcpip channels".to_string(),
            )
        }
        err => OpenErrorKind::Fatal(err.to_string()),
    }
}

/// Client mode (`-L`): `host:port` is the target as seen from the SSH host.
pub(super) async fn client_task(
    ctx: &mut ConnectionContext,
    tunnel: &SshTunnel,
    host: &Host,
    port: Port,
) -> ConnectionStatus {
    let emitter = ctx.emitter.clone();
    let target = format!("{}:{}", host.as_str(), port.get());
    emitter.info(
        file!(),
        line!(),
        format!(
            "Connecting to {target} through SSH {}:{}...",
            tunnel.host.as_str(),
            tunnel.port.get()
        ),
    );

    let session = tokio::select! {
        res =         open_session(&emitter, tunnel, OneShot::none()) => match res {
            Ok(session) => session,
            Err(message) => {
                emitter.error(file!(), line!(), message);
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            emitter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    let channel = tokio::select! {
        res = open_forward_channel(&emitter, &session, host, port) => match res {
            Ok(channel) => channel,
            Err(()) => {
                session.close().await;
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            emitter.info(file!(), line!(), "Connect attempt interrupted !");
            session.close().await;
            return ConnectionStatus::Disconnected;
        }
    };

    emitter.info(
        file!(),
        line!(),
        format!("Connected to {target} through SSH..."),
    );
    let status = serve(
        ctx,
        channel.into_stream(),
        &format!("Connected to {target} through SSH"),
    )
    .await;
    session.close().await;
    status
}

/// Server mode (`-R`): listens on the loopback of the SSH host, serves the first client.
pub(super) async fn server_task(
    ctx: &mut ConnectionContext,
    tunnel: &SshTunnel,
    port: Port,
) -> ConnectionStatus {
    let emitter = ctx.emitter.clone();
    let bind = format!("{REMOTE_BIND_ADDRESS}:{}", port.get());
    emitter.info(
        file!(),
        line!(),
        format!(
            "Starting server on {bind} of the SSH host {}:{}...",
            tunnel.host.as_str(),
            tunnel.port.get()
        ),
    );

    let (sender, mut receiver) = mpsc::channel(1);
    let session = tokio::select! {
        res =         open_session(&emitter, tunnel, OneShot::new(sender)) => match res {
            Ok(session) => session,
            Err(message) => {
                emitter.error(file!(), line!(), message);
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            emitter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    let forwarded = tokio::select! {
        res = session.tcpip_forward(REMOTE_BIND_ADDRESS, u32::from(port.get())) => res,
        _ = &mut ctx.shutdown => {
            emitter.info(file!(), line!(), "Connect attempt interrupted !");
            session.close().await;
            return ConnectionStatus::Disconnected;
        }
    };
    if let Err(err) = forwarded {
        let reason = match err {
            russh::Error::RequestDenied => {
                "the SSH server refused (port already in use, or TCP forwarding disabled)"
                    .to_string()
            }
            err => err.to_string(),
        };
        emitter.error(
            file!(),
            line!(),
            format!("Error while starting server on {bind} of the SSH host: {reason}"),
        );
        session.close().await;
        return ConnectionStatus::Error;
    }

    emitter.info(
        file!(),
        line!(),
        format!("Server listening on {bind} of the SSH host, waiting for a client..."),
    );
    ctx.status.send_replace(ConnectionStatus::Listening);
    emitter.emit_status(ConnectionStatus::Listening);

    let channel = tokio::select! {
        res = receiver.recv() => match res {
            Some(channel) => channel,
            None => {
                emitter.error(
                    file!(),
                    line!(),
                    "The SSH connection was closed while waiting for a client",
                );
                session.close().await;
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            emitter.info(
                file!(),
                line!(),
                "Server stopped while waiting for a client",
            );
            stop_forwarding(&emitter, &session, port).await;
            close_unserved(receiver).await;
            session.close().await;
            return ConnectionStatus::Disconnected;
        }
    };
    // Only one client is served: free the remote port; the gate already refuses any later one.
    close_unserved(receiver).await;
    stop_forwarding(&emitter, &session, port).await;

    emitter.info(
        file!(),
        line!(),
        format!("Client connected through SSH on {bind}..."),
    );
    let status = serve(
        ctx,
        channel.into_stream(),
        &format!("Accepted a connection on {bind} of the SSH host"),
    )
    .await;
    session.close().await;
    status
}

async fn stop_forwarding(emitter: &Emitter, session: &SshSession, port: Port) {
    let cancelled = timeout(
        DISCONNECT_TIMEOUT,
        session.cancel_tcpip_forward(REMOTE_BIND_ADDRESS, u32::from(port.get())),
    )
    .await;
    if !matches!(cancelled, Ok(Ok(()))) {
        emitter.warn(
            file!(),
            line!(),
            "The remote port forwarding could not be cancelled",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use russh::keys::parse_public_key_base64;

    use super::*;

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
        let err =
            verify_host_key(&dir.paths().known_hosts(), "vm", "vm", 22, &key(KEY_A)).unwrap_err();
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
}
