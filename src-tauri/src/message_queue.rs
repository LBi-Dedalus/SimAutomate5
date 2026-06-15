use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use crate::auto_response;
use crate::emitter::Emitter;
use crate::models::{AutoResponseConfig, LogLevel, MessageType, SendRequest};
use crate::translate::{self, ControlToken};

pub(crate) type SharedMessageQueue = Arc<MessageQueue>;

pub(crate) struct MessageQueue {
    emitter: Emitter,
    state: Mutex<MessageQueueState>,
    ready: Notify,
}

struct MessageQueueState {
    pending_messages: VecDeque<Vec<u8>>,
    waiting_for_ack: bool,
    auto_response: AutoResponseConfig,
}

impl MessageQueue {
    pub(crate) fn shared(
        emitter: Emitter,
        auto_response: AutoResponseConfig,
    ) -> SharedMessageQueue {
        Arc::new(Self {
            emitter,
            state: Mutex::new(MessageQueueState {
                pending_messages: VecDeque::new(),
                waiting_for_ack: false,
                auto_response,
            }),
            ready: Notify::new(),
        })
    }

    pub(crate) async fn update_auto_response(&self, config: AutoResponseConfig) {
        self.state.lock().await.auto_response = config;
    }

    pub(crate) async fn send_user_message(&self, payload: &SendRequest) {
        self.emitter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("sending user message={}", payload.message),
        );

        self.enqueue_message(&payload.message).await;
    }

    async fn enqueue_message(&self, message: &str) {
        self.state
            .lock()
            .await
            .pending_messages
            .extend(message.lines().map(translate::to_bytes));
        self.ready.notify_one();
    }

    pub(crate) async fn recv(&self) -> Vec<u8> {
        loop {
            let ready = self.ready.notified();

            {
                let mut state = self.state.lock().await;
                if !state.waiting_for_ack {
                    if let Some(message) = state.pending_messages.pop_front() {
                        state.waiting_for_ack = requires_ack(&message);
                        self.emitter.only_log(
                            LogLevel::Inf,
                            file!(),
                            line!(),
                            format!("releasing queued message bytes={}", message.len()),
                        );
                        return message;
                    }
                }
            }

            ready.await;
        }
    }

    pub(crate) async fn handle_received_message(&self, message: Vec<u8>) -> Result<(), String> {
        if message.is_empty() {
            return Err("Connection closed by peer".to_string());
        }

        self.emitter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("received message bytes={}", message.len()),
        );

        let visible = translate::to_human_readable(&message);
        self.emitter.emit_message(MessageType::Received, visible);

        if is_ack(&message) {
            self.handle_ack().await?;
        }

        self.send_auto_response(&message).await;
        Ok(())
    }

    async fn handle_ack(&self) -> Result<(), String> {
        let should_release = {
            let mut state = self.state.lock().await;

            if state.waiting_for_ack {
                state.waiting_for_ack = false;
                true
            } else {
                false
            }
        };

        if should_release {
            self.emitter.only_log(
                LogLevel::Inf,
                file!(),
                line!(),
                "received expected ACK, releasing queued messages",
            );
            self.ready.notify_one();
        }

        Ok(())
    }

    pub(crate) fn handle_sent_message(&self, msg: &[u8]) {
        self.emitter
            .emit_message(MessageType::Sent, translate::to_human_readable(msg));
    }

    async fn send_auto_response(&self, message: &[u8]) {
        let auto_response = self.state.lock().await.auto_response.clone();

        if let Some(response) = auto_response::build_auto_response(&auto_response, message) {
            self.emitter.only_log(
                LogLevel::Inf,
                file!(),
                line!(),
                format!("sending auto-response bytes={}", response.as_bytes().len()),
            );
            self.enqueue_message(&response).await;
        }
    }
}

fn requires_ack(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }

    line[0] == ControlToken::ENQ as u8 || line[0] == ControlToken::STX as u8
}

fn is_ack(message: &[u8]) -> bool {
    message.trim_ascii() == [ControlToken::ACK as u8]
}
