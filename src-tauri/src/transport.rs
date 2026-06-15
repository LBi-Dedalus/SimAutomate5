use tauri::async_runtime::{spawn, Receiver, Sender};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Duration};

use crate::emitter::Emitter;
use crate::message_queue::{MessageQueue, SharedMessageQueue};
use crate::models::{AutoResponseConfig, ConnectRequest, ConnectionStatus, SendRequest};
use crate::transport::Connection::Disconnected;

pub struct ConnectionManager {
    emitter: Emitter,
    auto_response: AutoResponseConfig,
    connection: Connection,
}

enum Connection {
    Disconnected,
    Started {
        message_queue: SharedMessageQueue,
        shutdown_sender: Sender<()>,
    },
}

impl ConnectionManager {
    pub fn new(emitter: Emitter) -> Self {
        Self {
            emitter: emitter.clone(),
            auto_response: AutoResponseConfig::default(),
            connection: Disconnected,
        }
    }

    pub async fn connect(&mut self, req: ConnectRequest) -> () {
        match &self.connection {
            Connection::Started { .. } => {
                self.emitter.warn(
                    file!(),
                    line!(),
                    "Connect command received while connection already started, ignoring",
                );
                self.emitter.emit_status(ConnectionStatus::Connected);
                return;
            }
            Disconnected => {
                let (tx_dc, rx_dc) = tauri::async_runtime::channel(1);
                let message_queue =
                    MessageQueue::shared(self.emitter.clone(), self.auto_response.clone());

                spawn(connect_and_read(
                    self.emitter.clone(),
                    format!("{}:{}", req.ip, req.port),
                    message_queue.clone(),
                    rx_dc,
                ));

                self.connection = Connection::Started {
                    message_queue,
                    shutdown_sender: tx_dc,
                };
            }
        }
    }

    pub async fn send_user_message(&mut self, payload: &SendRequest) {
        match &self.connection {
            Disconnected => self.emitter.error(
                file!(),
                line!(),
                "Could not send the message, are you connected ?",
            ),
            Connection::Started { message_queue, .. } => {
                message_queue.send_user_message(payload).await;
            }
        }
    }

    pub async fn update_auto_response(&mut self, config: AutoResponseConfig) {
        self.auto_response = config.clone();

        if let Connection::Started { message_queue, .. } = &self.connection {
            message_queue.update_auto_response(config).await;
        }
    }

    pub async fn disconnect(&mut self) {
        self.emitter.info(file!(), line!(), "Disconnecting...");

        match std::mem::replace(&mut self.connection, Disconnected) {
            Disconnected => {
                self.emitter
                    .info(file!(), line!(), "No connection, disconnect successful");
                self.emitter.emit_status(ConnectionStatus::Disconnected);
            }
            Connection::Started {
                shutdown_sender, ..
            } => match shutdown_sender.send(()).await {
                Ok(()) => (),
                Err(_) => {
                    self.emitter
                        .error(file!(), line!(), "Error while disconnecting");
                    self.emitter.emit_status(ConnectionStatus::Error);
                }
            },
        }
    }

    pub fn shutdown_now(&mut self) {
        if let Connection::Started {
            shutdown_sender, ..
        } = std::mem::replace(&mut self.connection, Disconnected)
        {
            shutdown_sender
                .try_send(())
                .expect("Could not shut down the connection properly !");
        }
    }
}

async fn connect_and_read(
    emitter: Emitter,
    addr: String,
    message_queue: SharedMessageQueue,
    mut dc_receiver: Receiver<()>,
) -> () {
    emitter.info(file!(), line!(), format!("Connecting to {}...", &addr));
    emitter.emit_status(ConnectionStatus::Connecting);

    let stream = tokio::select! {
        res = loop_till_connect(&emitter, addr.clone()) => {
            match res {
                Ok(tcp_stream) => tcp_stream,
                Err(_) => { return (); },
            }
        }
        _ = dc_receiver.recv() => {
            emitter.emit_status(ConnectionStatus::Disconnected);
            emitter.info(
                file!(),
                line!(),
                "Connect attempt interrupted !",
            );
            return ();
        }
    };

    let (mut reader, mut writer) = stream.into_split();
    emitter.info(file!(), line!(), format!("Connected to {}...", &addr));
    emitter.emit_status(ConnectionStatus::Connected);

    tokio::select! {
        result = read_loop(&mut reader, message_queue.clone()) => {
            if let Err(err) = result {
                emitter.emit_status(ConnectionStatus::Error);
                emitter.error(
                    file!(),
                    line!(),
                    format!("An error occurred while reading, disconnecting: {err}"),
                );
            }
        }
        result = send_loop(&mut writer, message_queue) => {
            if let Err(err) = result {
                emitter.emit_status(ConnectionStatus::Error);
                emitter.error(
                    file!(),
                    line!(),
                    format!("An error occurred while writing, disconnecting: {err}"),
                );
            }
        }
        _ = dc_receiver.recv() => {
            emitter.emit_status(ConnectionStatus::Disconnected);
            emitter.info(
                file!(),
                line!(),
                "Disconnected successfully",
            );
        }
    }

    let _ = writer.shutdown().await;
}

async fn loop_till_connect(emitter: &Emitter, addr: String) -> Result<TcpStream, ()> {
    let mut attempt = 1;
    loop {
        emitter.info(
            file!(),
            line!(),
            format!("connect attempt={} address={}", attempt, &addr),
        );

        let result = timeout(Duration::from_secs(1), TcpStream::connect(addr.clone())).await;

        match result {
            Ok(Ok(tcp_stream)) => {
                emitter.emit_status(ConnectionStatus::Connected);
                emitter.info(
                    file!(),
                    line!(),
                    format!("connect succeeded attempt={} address={}", attempt, &addr),
                );
                return Ok(tcp_stream);
            }
            Ok(Err(err)) => {
                emitter.error(
                    file!(),
                    line!(),
                    format!(
                        "connect failed attempt={} address={} error={}",
                        attempt,
                        &addr,
                        err.to_string()
                    ),
                );
                emitter.emit_status(ConnectionStatus::Error);
                return Err(());
            }
            Err(_err) => {
                emitter.warn(
                    file!(),
                    line!(),
                    format!(
                        "connect failed attempt={} address={}: time elapsed",
                        attempt, &addr
                    ),
                );
            }
        };

        sleep(Duration::from_secs(1)).await;
        attempt += 1;
    }
}

async fn read_loop(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    message_queue: SharedMessageQueue,
) -> Result<(), String> {
    let mut buffer = vec![0u8; 4096];

    loop {
        let len = reader
            .read(&mut buffer)
            .await
            .map_err(|err| err.to_string())?;
        let res = Vec::from_iter(buffer[..len].iter().copied());
        message_queue.handle_received_message(res).await?;
    }
}

async fn send_loop(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    message_queue: SharedMessageQueue,
) -> Result<(), String> {
    loop {
        let msg = message_queue.recv().await;
        let mut pos = 0;

        while pos < msg.len() {
            let len = writer
                .write(&msg[pos..])
                .await
                .map_err(|err| err.to_string())?;
            if len == 0 {
                return Err("Connection closed by peer".to_string());
            }
            pos += len;
        }

        message_queue.handle_sent_message(&msg);
    }
}
