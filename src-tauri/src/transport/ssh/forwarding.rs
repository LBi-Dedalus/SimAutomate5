//! Channels and port forwarding: the `direct-tcpip` channel of the client mode (`-L`) and the
//! `tcpip-forward` bookkeeping of the server mode (`-R`).

use russh::client::Msg;
use russh::{Channel, ChannelOpenFailure};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Duration};

use super::session::{SshSession, DISCONNECT_TIMEOUT};
use crate::models::{Host, LogLevel, Port};
use crate::reporting::ConnectionReporter;

const CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(1);
const CHANNEL_RETRY_DELAY: Duration = Duration::from_secs(1);
/// Loopback of the SSH host: the server mode never opens a port to the network.
pub(super) const REMOTE_BIND_ADDRESS: &str = "127.0.0.1";

/// Opens the `direct-tcpip` channel. The request is sent once and never abandoned (dropping the
/// future would not cancel it, and a late confirmation would leave an unserved channel): the same
/// future is polled until it resolves, with a warning every second while the target is slow.
/// An explicit refusal, or the death of the SSH session, is fatal.
pub(super) async fn open_forward_channel(
    reporter: &ConnectionReporter,
    session: &SshSession,
    host: &Host,
    port: Port,
) -> Result<Channel<Msg>, ()> {
    let target = format!("{}:{}", host.as_str(), port.get());
    let mut attempt = 1;
    loop {
        reporter.only_log(
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
                    reporter.warn(
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
                reporter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("ssh channel succeeded attempt={attempt} address={target}"),
                );
                return Ok(channel);
            }
            Err(err) => match classify_open_error(&err) {
                OpenErrorKind::Retry => {
                    reporter.warn(
                        file!(),
                        line!(),
                        format!(
                            "Target {target} not reachable from the SSH host ({err}), \
                             retrying in 1s: attempt={attempt}"
                        ),
                    );
                }
                OpenErrorKind::Fatal(message) => {
                    reporter.error(
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
pub(super) enum OpenErrorKind {
    /// The target does not accept (yet): open a new channel after a delay.
    Retry,
    /// Retrying is pointless or the session is broken: end with this message.
    Fatal(String),
}

pub(super) fn classify_open_error(err: &russh::Error) -> OpenErrorKind {
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

/// Explicitly closes the channels nobody will serve (a bare russh channel does not send a close
/// when dropped).
pub(super) async fn close_unserved(mut receiver: mpsc::Receiver<Channel<Msg>>) {
    receiver.close();
    while let Ok(channel) = receiver.try_recv() {
        let _ = channel.close().await;
    }
}

/// Cancels the remote `tcpip-forward`, warning when that fails or takes too long.
pub(super) async fn stop_forwarding(
    reporter: &ConnectionReporter,
    session: &SshSession,
    port: Port,
) {
    let cancelled = timeout(
        DISCONNECT_TIMEOUT,
        session.cancel_tcpip_forward(REMOTE_BIND_ADDRESS, u32::from(port.get())),
    )
    .await;
    if !matches!(cancelled, Ok(Ok(()))) {
        reporter.warn(
            file!(),
            line!(),
            "The remote port forwarding could not be cancelled",
        );
    }
}
