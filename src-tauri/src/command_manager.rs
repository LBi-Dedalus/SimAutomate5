use crate::{
    logger::AppLogger,
    models::{Command, LogLevel},
    transport::ConnectionManager,
};
use tauri::{
    async_runtime::{spawn, Receiver, Sender},
    AppHandle,
};

pub struct CommandQueue {
    sender: Option<Sender<Command>>,
}

impl CommandQueue {
    pub fn start(app: &AppHandle, logger: AppLogger) -> Self {
        logger.log_backend(LogLevel::Inf, file!(), line!(), "starting connection queue");

        let (tx, rx) = tauri::async_runtime::channel::<Command>(100);
        spawn(receive_loop(tx.clone(), rx, app.clone(), logger));

        Self { sender: Some(tx) }
    }

    pub async fn send(&self, command: Command) -> Result<(), String> {
        match &self.sender {
            None => Err("Queue closed".to_string()),
            Some(sender) => match sender.send(command).await {
                Err(_err) => Err("Could not send command".to_string()),
                Ok(_) => Ok(()),
            },
        }
    }

    pub fn close(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.blocking_send(Command::Shutdown);
        }
    }
}

async fn receive_loop(
    sender: Sender<Command>,
    mut queue: Receiver<Command>,
    app: AppHandle,
    logger: AppLogger,
) {
    logger.log_backend(LogLevel::Inf, file!(), line!(), "receive loop started");

    let mut connection = ConnectionManager::new(app.clone(), logger.clone());

    while let Some(message) = queue.recv().await {
        match message {
            Command::Connect(req) => connection.connect(req, sender.clone()),
            Command::Disconnect => connection.disconnect().await,
            Command::SendMessage(send_request) => {
                connection.send_user_message(&send_request).await;
            }
            Command::MessageReceived(message_res) => {
                if let Err(err) = connection.handle_message(message_res).await {
                    logger.log_backend(
                        LogLevel::Err,
                        file!(),
                        line!(),
                        format!("message handling failed: {err}"),
                    );
                    connection.disconnect().await;
                    queue.close();
                }
            }
            Command::Shutdown => {
                connection.disconnect().await;
                queue.close();
            }
        }
    }

    logger.log_backend(LogLevel::Inf, file!(), line!(), "receive loop terminated");
}
