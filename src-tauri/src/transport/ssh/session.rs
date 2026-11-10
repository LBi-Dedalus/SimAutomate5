//! The SSH connection: handshake, host key handler, authentication and the session lifecycle
//! (every transport is cut, every session disconnected, whatever the way out).

use std::borrow::Cow;
use std::io;
use std::ops::Deref;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use russh::client::{self, Handle, Msg};
use russh::keys::known_hosts::known_host_keys_path;
use russh::keys::{Algorithm, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelOpenFailure, Disconnect, Preferred};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

use super::settings::{
    load_host_settings, load_key, local_user_from_env, preferred_host_key_algorithms, resolve_key,
    resolve_user, verify_host_key, SshPaths,
};
use crate::models::SshTunnel;
use crate::reporting::ConnectionReporter;
use crate::transport::cleanup::{wait_for_force, AttemptCleanup};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_MAX: usize = 3;
pub(super) const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(super) enum SshError {
    Russh(russh::Error),
    /// The host key was refused; the message tells the user what to do.
    HostKey(String),
}

impl From<russh::Error> for SshError {
    fn from(err: russh::Error) -> Self {
        Self::Russh(err)
    }
}

pub(super) fn describe_connect_error(host: &str, port: u16, err: SshError) -> String {
    match err {
        SshError::HostKey(message) => message,
        SshError::Russh(err) => format!("SSH connection to {host}:{port} failed: {err}"),
    }
}

pub(super) struct SshHandler {
    /// Name connected to (after the `Hostname` of the SSH config).
    pub(super) host: String,
    pub(super) alias: String,
    pub(super) port: u16,
    pub(super) known_hosts: PathBuf,
    /// Server mode: receives the first channel opened by the SSH host for the forwarded port.
    pub(super) forwarded: Option<oneshot::Sender<Channel<Msg>>>, // OneShot<Channel<Msg>>,
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
        let delivered = match self.forwarded.take() {
            Some(sender) => sender.send(channel),
            None => Err(channel),
        };

        match delivered {
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

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

/// Cuts a `CancellableStream`: pending and later reads see EOF, writes fail.
#[derive(Clone, Default)]
pub(super) struct CancelHandle(Arc<CancelState>);

impl CancelHandle {
    pub(super) fn cancel(&self) {
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

pub(super) struct CancellableStream<S> {
    inner: S,
    cancel: CancelHandle,
}

impl<S> CancellableStream<S> {
    pub(super) fn new(inner: S) -> (Self, CancelHandle) {
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
        self.cancel.register(cx.waker());
        if self.cancel.is_cancelled() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// TCP connect + SSH handshake within `budget`. The transport is owned here so that any early
/// exit (timeout, error, the future being dropped) cuts it and ends the russh session task.
pub(super) async fn connect_transport(
    config: Arc<client::Config>,
    host: &str,
    port: u16,
    handler: SshHandler,
    budget: Duration,
    cleanup: &AttemptCleanup,
    reporter: &ConnectionReporter,
) -> Result<SshSession, String> {
    let attempt = async {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|err| describe_connect_error(host, port, SshError::Russh(err.into())))?;
        if config.nodelay {
            let _ = tcp.set_nodelay(true);
        }
        let (stream, cancel) = CancellableStream::new(tcp);
        let guard = CancelOnDrop(Some(cancel.clone()));
        let (connected, connection) = oneshot::channel();
        let session_cleanup = cleanup.clone();
        let session_reporter = reporter.clone();
        let host = host.to_string();
        // Never abandon connect_stream after it has spawned russh's task: on cancellation
        // the guard cuts its stream, while this tracked job still observes its completion.
        cleanup.spawn(async move {
            let result = client::connect_stream(config, stream, handler)
                .await
                .map(|handle| SshSession::new(handle, cancel, session_cleanup, session_reporter))
                .map_err(|err| describe_connect_error(&host, port, err));
            if let Err(undelivered) = connected.send(result) {
                drop(undelivered);
            }
            Ok(())
        });
        let session = connection
            .await
            .map_err(|err| format!("The SSH handshake task ended without a result: {err}"))??;
        guard.disarm();
        Ok(session)
    };
    timeout(budget, attempt).await.map_err(|_| {
        format!(
            "Timed out connecting to {host}:{port} after {} s",
            budget.as_secs()
        )
    })?
}

/// Explicit close and Drop transfer the handle to the same attempt-owned cleanup tracker.
/// Cutting the transport is a safety net; only awaiting the handle confirms local completion.
pub(super) struct SshSession {
    handle: Option<Handle<SshHandler>>,
    transport: CancelHandle,
    cleanup: AttemptCleanup,
    reporter: ConnectionReporter,
}

impl SshSession {
    fn new(
        handle: Handle<SshHandler>,
        transport: CancelHandle,
        cleanup: AttemptCleanup,
        reporter: ConnectionReporter,
    ) -> Self {
        Self {
            handle: Some(handle),
            transport,
            cleanup,
            reporter,
        }
    }

    pub(super) async fn close(mut self) {
        if let Some(completion) = self.start_close() {
            if let Err(err) = completion.await {
                self.reporter.error(
                    file!(),
                    line!(),
                    format!("The SSH cleanup task ended without confirming completion: {err}"),
                );
            }
        }
    }

    fn start_close(&mut self) -> Option<oneshot::Receiver<()>> {
        let handle = self.handle.take()?;
        Some(self.cleanup.spawn(disconnect(
            handle,
            self.transport.clone(),
            self.cleanup.force_stop(),
            self.reporter.clone(),
        )))
    }
}

async fn disconnect(
    mut handle: Handle<SshHandler>,
    transport: CancelHandle,
    mut force_stop: tokio::sync::watch::Receiver<bool>,
    reporter: ConnectionReporter,
) -> Result<(), String> {
    let _guard = CancelOnDrop(Some(transport.clone()));
    let graceful = async {
        if let Err(err) = handle.disconnect(Disconnect::ByApplication, "", "en").await {
            reporter.warn(
                file!(),
                line!(),
                format!("SSH disconnect request failed: {err}"),
            );
        }
        (&mut handle).await
    };
    tokio::select! {
        biased;
        _ = wait_for_force(&mut force_stop) => {}
        result = timeout(DISCONNECT_TIMEOUT, graceful) => {
            match result {
                Ok(Ok(())) | Ok(Err(SshError::Russh(russh::Error::Disconnect))) => return Ok(()),
                Ok(Err(err)) => return Err(format!("SSH shutdown failed: {err:?}")),
                Err(_) => reporter.warn(
                    file!(),
                    line!(),
                    "SSH graceful shutdown timed out; cancelling its transport and waiting for cleanup",
                ),
            }
        }
    }
    transport.cancel();
    if let Err(err) = handle.await {
        reporter.warn(
            file!(),
            line!(),
            format!("The SSH task ended after transport cancellation: {err:?}"),
        );
    }
    Ok(())
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
        if self.handle.is_none() {
            self.transport.cancel();
        } else if tokio::runtime::Handle::try_current().is_ok() {
            self.start_close();
        } else {
            self.transport.cancel();
            self.reporter.error(
                file!(),
                line!(),
                "The SSH transport was cancelled, but cleanup could not be observed without a runtime",
            );
        }
    }
}

/// Connects and authenticates, once and without retry (every failure here is fatal).
pub(super) async fn open_session(
    reporter: &ConnectionReporter,
    tunnel: &SshTunnel,
    forwarded: Option<oneshot::Sender<Channel<Msg>>>,
    cleanup: &AttemptCleanup,
) -> Result<SshSession, String> {
    let paths = SshPaths::from_env()?;
    let alias = tunnel.host.as_str();
    let port = tunnel.port.get();

    let (settings, warning) = load_host_settings(&paths.config(), alias);
    if let Some(warning) = warning {
        reporter.warn(file!(), line!(), warning);
    }
    let host = settings
        .hostname
        .clone()
        .unwrap_or_else(|| alias.to_string());
    let user = resolve_user(tunnel.user.as_ref(), &settings, local_user_from_env())?;
    let key_path = resolve_key(&settings, &paths)?;
    reporter.info(
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

    let mut session = connect_transport(
        Arc::new(config),
        &host,
        port,
        handler,
        CONNECT_TIMEOUT,
        cleanup,
        reporter,
    )
    .await?;
    reporter.info(
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
    reporter.info(
        file!(),
        line!(),
        format!("SSH: authenticated as {user} on {host}:{port}"),
    );
    Ok(session)
}
