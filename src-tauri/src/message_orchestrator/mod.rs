mod composer;
mod state;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time::{sleep_until, Instant};

use crate::auto_reply::{AutoReplyOutcome, AutoReplyRuleSet};
use crate::models::{LogLevel, MessageType, SendRequest};
use crate::protocol::frames::{FrameBuffer, FrameEvent};
use crate::reporting::ConnectionReporter;
use crate::translate;

use composer::split_message;
use state::{AckOrigin, Discarded, MessageQueueState, ACK_TIMEOUT};

pub(crate) type SharedMessageOrchestrator = Arc<MessageOrchestrator>;

/// Largest user message accepted by `send_user_message`.
const MAX_USER_PAYLOAD: usize = 1024 * 1024;

/// Stream state of the bytes received on one connection attempt.
struct ReceiveState {
    /// Reassembles multibyte characters split across reads.
    decoder: translate::ControlTokenDecoder,
    /// Extracts the complete protocol messages (ACK, NAK, EOT, ENQ, frames...).
    frames: FrameBuffer,
}

/// One orchestrator per connection attempt, and the single entry point of the bytes received
/// on it (`on_received`): it reports them, releases or aborts the send queue on ACK / NAK / EOT
/// and schedules the automatic replies. It also holds the user messages (released one at a
/// time, an ASTM ENQ/STX waits for its ACK) and the scheduled automatic replies.
///
/// Scheduling policy: a due automatic reply goes out before any queued user message. While an
/// ACK is awaited, only a PURE automatic `<ACK>` / `<NAK>` may go out: answering the peer's
/// ENQ/frame must never wait for the peer to answer ours (that would deadlock a bidirectional
/// exchange), but no other frame bypasses the wait.
pub(crate) struct MessageOrchestrator {
    reporter: ConnectionReporter,
    state: Mutex<MessageQueueState>,
    // Separate lock so decoding never contends with the send loop.
    receive: Mutex<ReceiveState>,
    ready: Notify,
}

impl MessageOrchestrator {
    pub(crate) fn shared(
        reporter: ConnectionReporter,
        rules: Arc<AutoReplyRuleSet>,
    ) -> SharedMessageOrchestrator {
        Self::with_ack_timeout(reporter, rules, ACK_TIMEOUT)
    }

    /// Same as [`Self::shared`] with a custom ACK wait (tests use short durations).
    pub(crate) fn with_ack_timeout(
        reporter: ConnectionReporter,
        rules: Arc<AutoReplyRuleSet>,
        ack_timeout: Duration,
    ) -> SharedMessageOrchestrator {
        Arc::new(Self {
            reporter,
            state: Mutex::new(MessageQueueState::new(rules, ack_timeout)),
            receive: Mutex::new(ReceiveState {
                decoder: translate::ControlTokenDecoder::new(),
                frames: FrameBuffer::new(),
            }),
            ready: Notify::new(),
        })
    }

    /// Replaces the rules; replies scheduled by the previous rules are dropped.
    pub(crate) async fn update_rules(&self, rules: Arc<AutoReplyRuleSet>) {
        // Scope the lock so it is released before logging and notifying.
        let dropped = {
            let mut state = self.state.lock().await;
            let dropped = state.clear_replies();
            state.rules = rules;
            dropped
        };
        if dropped > 0 {
            self.reporter.only_log(
                LogLevel::Inf,
                file!(),
                line!(),
                format!("auto reply rules changed, dropped {dropped} pending replies"),
            );
        }
        self.ready.notify_one();
    }

    /// The attempt is over: pending user messages and automatic replies are discarded.
    pub(crate) async fn close(&self) {
        let mut state = self.state.lock().await;
        state.close();
    }

    pub(crate) async fn send_user_message(&self, payload: &SendRequest) -> Result<(), String> {
        if payload.message.len() > MAX_USER_PAYLOAD {
            return Err(format!(
                "Message too large ({} bytes, maximum {MAX_USER_PAYLOAD})",
                payload.message.len()
            ));
        }
        self.reporter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!(
                "sending user message lines={} chars={}",
                payload.message.lines().count(),
                payload.message.chars().count()
            ),
        );

        self.enqueue_message(&payload.message).await
    }

    async fn enqueue_message(&self, message: &str) -> Result<(), String> {
        let items = split_message(message);
        // Scope the lock so it is released before notifying.
        {
            let mut state = self.state.lock().await;
            if state.closed {
                return Ok(());
            }
            state.push_user(items)?;
        }
        self.ready.notify_one();
        Ok(())
    }

    pub(crate) async fn recv(&self) -> Vec<u8> {
        loop {
            let ready = self.ready.notified();
            let wake_at;

            {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                if state.ack_overdue(now) {
                    let dropped = state.reset_sending();
                    let timeout = state.ack_timeout;
                    // Drop the lock
                    drop(state);
                    self.report_ack_timeout(dropped, timeout);
                    continue;
                }
                if let Some(frame) = state.take_due(now) {
                    self.reporter.only_log(
                        LogLevel::Inf,
                        file!(),
                        line!(),
                        format!("releasing automatic reply bytes={}", frame.len()),
                    );
                    return frame;
                }
                if !state.waiting_for_ack() {
                    if let Some(message) = state.pop_user() {
                        state.start_ack_wait(&message, now, AckOrigin::User);
                        self.reporter.only_log(
                            LogLevel::Inf,
                            file!(),
                            line!(),
                            format!("releasing queued message bytes={}", message.len()),
                        );
                        return message;
                    }
                }
                let ack_wake = state.ack_wait.map(|wait| wait.deadline);
                wake_at = match (state.next_wakeup(now), ack_wake) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }

            // Never sleeps while holding the lock or in the receive path: a delayed reply only
            // makes THIS wait end earlier than the next notification.
            match wake_at {
                Some(deadline) => {
                    tokio::select! {
                        _ = ready => {}
                        _ = sleep_until(deadline) => {}
                    }
                }
                None => ready.await,
            }
        }
    }

    /// Entry point of the bytes of one socket read: reports them as is, then handles the
    /// protocol messages they complete (ACK releases the send queue, NAK / EOT abort it, the
    /// others go through the auto reply rules). An empty read means the peer closed.
    pub(crate) async fn on_received(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            self.on_closed().await;
            return Err("Connection closed by peer".to_string());
        }

        self.reporter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("received message bytes={}", bytes.len()),
        );

        let (visible, events) = {
            let mut receive = self.receive.lock().await;
            (receive.decoder.decode(bytes), receive.frames.feed(bytes))
        };
        self.reporter
            .emit_message_with_raw(MessageType::Received, visible, bytes.to_vec());
        for event in events {
            self.handle_frame(event).await;
        }
        Ok(())
    }

    /// The peer closed: the dangling partial character is dropped (not emitted as a message)
    /// and only its size is logged.
    async fn on_closed(&self) {
        let pending = {
            let mut receive = self.receive.lock().await;
            let pending = receive.decoder.pending_len();
            let _ = receive.decoder.finish();
            pending
        };
        if pending > 0 {
            self.reporter.only_log(
                LogLevel::Inf,
                file!(),
                line!(),
                format!("connection closed with incomplete UTF-8 sequence bytes={pending}"),
            );
        }
    }
    fn report_ack_timeout(&self, dropped: Discarded, timeout: Duration) {
        self.reporter.warn(
            file!(),
            line!(),
            format!(
                "No ACK received within {} ms: sending stopped, send queue cleared ({})",
                timeout.as_millis(),
                dropped.describe()
            ),
        );
    }

    /// A standalone NAK or EOT: the peer refuses or ends the transmission. Nothing is resent
    /// and nothing advances: the send queue is cleared (only when something was in flight or
    /// queued, otherwise the byte is just logged).
    async fn handle_abort(&self, what: &str) {
        let outcome = {
            let mut state = self.state.lock().await;
            if state.closed {
                return;
            }
            let was_waiting = state.waiting_for_ack();
            let dropped = state.reset_sending();
            (was_waiting, dropped)
        };
        match outcome {
            (false, dropped) if dropped.total() == 0 => self.reporter.only_log(
                LogLevel::Inf,
                file!(),
                line!(),
                format!("received {what} with nothing to send"),
            ),
            (_, dropped) => self.reporter.warn(
                file!(),
                line!(),
                format!(
                    "Received {what}: sending stopped, send queue cleared ({})",
                    dropped.describe()
                ),
            ),
        }
        self.ready.notify_one();
    }

    async fn handle_ack(&self) -> Result<(), String> {
        let (should_release, timed_out) = {
            let mut state = self.state.lock().await;

            if state.ack_overdue(Instant::now()) {
                // Too late: the timeout wins, this ACK is ignored.
                let timeout = state.ack_timeout;
                (false, Some((state.reset_sending(), timeout)))
            } else if state.waiting_for_ack() {
                state.stop_ack_wait();
                (true, None)
            } else {
                (false, None)
            }
        };

        if let Some((dropped, timeout)) = timed_out {
            self.report_ack_timeout(dropped, timeout);
            self.ready.notify_one();
        }
        if should_release {
            self.reporter.only_log(
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
        self.reporter.emit_message_with_raw(
            MessageType::Sent,
            translate::ControlTokenDecoder::decode_all(msg),
            msg.to_vec(),
        );
    }

    /// A complete protocol message (see `frames`): evaluates the rules once and schedules the
    /// reply. Failures are reported with metadata only (never the peer's content).
    async fn handle_frame(&self, event: FrameEvent) {
        // The ACK of our own transmission releases the send queue whatever the rules say.
        if event == FrameEvent::Ack {
            let _ = self.handle_ack().await;
            return;
        }
        // NAK / EOT stop the send queue whatever the rules say.
        match event {
            FrameEvent::Nak => return self.handle_abort("NAK").await,
            FrameEvent::Eot => return self.handle_abort("EOT").await,
            _ => {}
        }
        let mut state = self.state.lock().await;
        if state.closed || !state.rules.is_enabled() {
            return;
        }
        match &event {
            FrameEvent::Malformed(reason) => {
                drop(state);
                self.reporter.warn(
                    file!(),
                    line!(),
                    format!("Auto reply: {reason}; no reply was sent"),
                );
                return;
            }
            FrameEvent::Overflow => {
                drop(state);
                self.reporter.error(
                    file!(),
                    line!(),
                    "Auto reply: an incoming message is too large and was ignored; no reply was sent",
                );
                return;
            }
            _ => {}
        }

        match state.rules.evaluate(&event) {
            AutoReplyOutcome::NoMatch => {
                drop(state);
                self.reporter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    "no auto reply rule matched",
                );
            }
            AutoReplyOutcome::Suppressed { rule_name } => {
                drop(state);
                self.reporter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("auto reply rule \"{rule_name}\" matched: no auto reply"),
                );
            }
            AutoReplyOutcome::Failed { rule_name, reason } => {
                drop(state);
                self.reporter.error(
                    file!(),
                    line!(),
                    format!("Auto reply rule \"{rule_name}\" could not reply: {reason}"),
                );
            }
            AutoReplyOutcome::Reply(reply) => {
                let due = Instant::now() + reply.delay;
                let frames = reply.frames.len();
                let pushed = state.push_replies(due, reply.frames);
                drop(state);
                if let Err(e) = pushed {
                    self.reporter.error(
                        file!(),
                        line!(),
                        format!("Auto reply rule \"{}\" dropped: {e}", reply.rule_name),
                    );
                    return;
                }
                self.reporter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!(
                        "auto reply rule \"{}\" matched, {frames} frame(s) due in {} ms",
                        reply.rule_name,
                        reply.delay.as_millis()
                    ),
                );
                self.ready.notify_one();
            }
        }
    }
}
