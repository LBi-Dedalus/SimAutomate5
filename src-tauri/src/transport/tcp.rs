use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout, Duration};

use super::streams::{serve, ConnectionContext};
use crate::models::{ConnectionStatus, LogLevel};
use crate::reporting::ConnectionReporter;

pub(super) async fn client_task(ctx: &mut ConnectionContext, addr: String) -> ConnectionStatus {
    let reporter = ctx.reporter.clone();
    reporter.info(file!(), line!(), format!("Connecting to {}...", addr));

    let stream = tokio::select! {
        res = loop_till_connect(&reporter, addr.clone()) => {
            match res {
                Ok(tcp_stream) => tcp_stream,
                Err(_) => return ConnectionStatus::Error,
            }
        }
        _ = &mut ctx.shutdown => {
            reporter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    reporter.info(file!(), line!(), format!("Connected to {}...", addr));
    serve(ctx, stream, &format!("Connected to {}", addr)).await
}

pub(super) async fn server_task(ctx: &mut ConnectionContext, port: u16) -> ConnectionStatus {
    let reporter = ctx.reporter.clone();
    reporter.info(
        file!(),
        line!(),
        format!("Starting server on port {}...", port),
    );
    let addr = format!("0.0.0.0:{}", port);

    let listener = tokio::select! {
        res = TcpListener::bind(addr.clone()) => {
            match res {
                Ok(listener) => listener,
                Err(err) => {
                    reporter.error(
                        file!(),
                        line!(),
                        format!("Error while starting server {}", err),
                    );
                    return ConnectionStatus::Error;
                }
            }
        }
        _ = &mut ctx.shutdown => {
            reporter.info(file!(), line!(), "Connect attempt interrupted !");
            return ConnectionStatus::Disconnected;
        }
    };

    // The port is really bound: only now is the server ready.
    reporter.info(
        file!(),
        line!(),
        format!("Server listening on {}, waiting for a client...", addr),
    );
    ctx.status.send_replace(ConnectionStatus::Listening);
    reporter.emit_status(ConnectionStatus::Listening);

    let stream = tokio::select! {
        res = listener.accept() => {
            match res {
                Ok((tcp_stream, client_addr)) => {
                    reporter.info(
                        file!(),
                        line!(),
                        format!("Client connected address={}", client_addr),
                    );
                    tcp_stream
                }
                Err(err) => {
                    reporter.error(
                        file!(),
                        line!(),
                        format!("Error while accepting connection {}", err),
                    );
                    return ConnectionStatus::Error;
                }
            }
        }
        _ = &mut ctx.shutdown => {
            reporter.info(
                file!(),
                line!(),
                "Server stopped while waiting for a client",
            );
            return ConnectionStatus::Disconnected;
        }
    };
    // Only one client is served: free the port as soon as it is accepted.
    drop(listener);

    let peer_addr = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or("unknown".to_string());
    reporter.info(
        file!(),
        line!(),
        format!(
            "Client from {} connected, listening on {}...",
            peer_addr, addr
        ),
    );
    serve(
        ctx,
        stream,
        &format!("Accepted a connection from {} on {}", peer_addr, addr),
    )
    .await
}

async fn loop_till_connect(reporter: &ConnectionReporter, addr: String) -> Result<TcpStream, ()> {
    let mut attempt = 1;
    loop {
        reporter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("connect attempt={} address={}", attempt, addr),
        );

        let result = timeout(Duration::from_secs(1), TcpStream::connect(addr.clone())).await;

        match result {
            Ok(Ok(tcp_stream)) => {
                reporter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("connect succeeded attempt={} address={}", attempt, addr),
                );
                return Ok(tcp_stream);
            }
            Ok(Err(err)) => {
                reporter.error(
                    file!(),
                    line!(),
                    format!("Connection to {} failed: {}", addr, err),
                );
                return Err(());
            }
            Err(_err) => {
                reporter.warn(
                    file!(),
                    line!(),
                    format!("Connection to {} timed out: attempt={}", addr, attempt),
                );
            }
        };

        sleep(Duration::from_secs(1)).await;
        attempt += 1;
    }
}
