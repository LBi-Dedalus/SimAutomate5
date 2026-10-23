//! Tunnelling the simulator socket through SSH: `-L` (`client_task`, `direct-tcpip`) and `-R`
//! (`server_task`, `tcpip-forward` on the loopback of the SSH host).
//!
//! The SSH connection and authentication are attempted once; host keys are only ever read from
//! the user's `known_hosts` (never written); authentication uses an unencrypted private key file.

mod forwarding;
mod session;
mod settings;

use tokio::sync::mpsc;

use super::streams::{serve, ConnectionContext};
use crate::models::{ConnectionStatus, Host, Port, SshTunnel};
use forwarding::{close_unserved, open_forward_channel, stop_forwarding, REMOTE_BIND_ADDRESS};
use session::{open_session, OneShot};

/// Client mode (`-L`): `host:port` is the target as seen from the SSH host.
pub(super) async fn client_task(
    ctx: &mut ConnectionContext,
    tunnel: &SshTunnel,
    host: &Host,
    port: Port,
) -> ConnectionStatus {
    let reporter = ctx.reporter.clone();
    let target = format!("{}:{}", host.as_str(), port.get());
    reporter.info(
        file!(),
        line!(),
        format!(
            "Connecting to {target} through SSH {}:{}...",
            tunnel.host.as_str(),
            tunnel.port.get()
        ),
    );

    let session = tokio::select! {
        res =         open_session(&reporter, tunnel, OneShot::none()) => match res {
            Ok(session) => session,
            Err(message) => {
                reporter.error(file!(), line!(), message);
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            reporter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    let channel = tokio::select! {
        res = open_forward_channel(&reporter, &session, host, port) => match res {
            Ok(channel) => channel,
            Err(()) => {
                session.close().await;
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            reporter.info(file!(), line!(), "Connect attempt interrupted !");
            session.close().await;
            return ConnectionStatus::Disconnected;
        }
    };

    reporter.info(
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
    let reporter = ctx.reporter.clone();
    let bind = format!("{REMOTE_BIND_ADDRESS}:{}", port.get());
    reporter.info(
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
        res =         open_session(&reporter, tunnel, OneShot::new(sender)) => match res {
            Ok(session) => session,
            Err(message) => {
                reporter.error(file!(), line!(), message);
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            reporter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    let forwarded = tokio::select! {
        res = session.tcpip_forward(REMOTE_BIND_ADDRESS, u32::from(port.get())) => res,
        _ = &mut ctx.shutdown => {
            reporter.info(file!(), line!(), "Connect attempt interrupted !");
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
        reporter.error(
            file!(),
            line!(),
            format!("Error while starting server on {bind} of the SSH host: {reason}"),
        );
        session.close().await;
        return ConnectionStatus::Error;
    }

    reporter.info(
        file!(),
        line!(),
        format!("Server listening on {bind} of the SSH host, waiting for a client..."),
    );
    ctx.status.send_replace(ConnectionStatus::Listening);
    reporter.emit_status(ConnectionStatus::Listening);

    let channel = tokio::select! {
        res = receiver.recv() => match res {
            Some(channel) => channel,
            None => {
                reporter.error(
                    file!(),
                    line!(),
                    "The SSH connection was closed while waiting for a client",
                );
                session.close().await;
                return ConnectionStatus::Error;
            }
        },
        _ = &mut ctx.shutdown => {
            reporter.info(
                file!(),
                line!(),
                "Server stopped while waiting for a client",
            );
            stop_forwarding(&reporter, &session, port).await;
            close_unserved(receiver).await;
            session.close().await;
            return ConnectionStatus::Disconnected;
        }
    };
    // Only one client is served: free the remote port; the gate already refuses any later one.
    close_unserved(receiver).await;
    stop_forwarding(&reporter, &session, port).await;

    reporter.info(
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

#[cfg(test)]
mod tests;
