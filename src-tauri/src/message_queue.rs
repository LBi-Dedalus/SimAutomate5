use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use tokio::time::{sleep_until, Instant};

use crate::auto_reply::{Outcome, RuleSet};
use crate::emitter::Emitter;
use crate::frames::FrameEvent;
use crate::models::{LogLevel, MessageType, SendRequest};
use crate::translate::{self, ControlToken};

pub(crate) type SharedMessageQueue = Arc<MessageQueue>;

/// One queue per connection attempt. It holds the user messages (released one at a time, an
/// ASTM ENQ/STX waits for its ACK) and the scheduled automatic replies.
///
/// Scheduling policy: a due automatic reply goes out before any queued user message. While an
/// ACK is awaited, only a PURE automatic `<ACK>` / `<NAK>` may go out: answering the peer's
/// ENQ/frame must never wait for the peer to answer ours (that would deadlock a bidirectional
/// exchange), but no other frame bypasses the wait.
pub(crate) struct MessageQueue {
    emitter: Emitter,
    state: Mutex<MessageQueueState>,
    ready: Notify,
}

struct ScheduledReply {
    due: Instant,
    seq: u64,
    frame: Vec<u8>,
}

struct MessageQueueState {
    pending_messages: VecDeque<Vec<u8>>,
    waiting_for_ack: bool,
    rules: Arc<RuleSet>,
    scheduled: Vec<ScheduledReply>,
    next_seq: u64,
    /// Set when the attempt ended: nothing is queued or scheduled any more.
    closed: bool,
}

impl MessageQueueState {
    /// Earliest due reply that may be released now.
    fn take_due(&mut self, now: Instant) -> Option<Vec<u8>> {
        let waiting = self.waiting_for_ack;
        let index = self
            .scheduled
            .iter()
            .enumerate()
            .filter(|(_, reply)| reply.due <= now && (!waiting || is_control_reply(&reply.frame)))
            .min_by_key(|(_, reply)| (reply.due, reply.seq))
            .map(|(index, _)| index)?;
        let reply = self.scheduled.remove(index);
        if !is_control_reply(&reply.frame) {
            self.waiting_for_ack = requires_ack(&reply.frame);
        }
        Some(reply.frame)
    }

    /// Earliest future moment at which a reply becomes releasable.
    fn next_wakeup(&self, now: Instant) -> Option<Instant> {
        let waiting = self.waiting_for_ack;
        self.scheduled
            .iter()
            .filter(|reply| reply.due > now && (!waiting || is_control_reply(&reply.frame)))
            .map(|reply| reply.due)
            .min()
    }
}

impl MessageQueue {
    pub(crate) fn shared(emitter: Emitter, rules: Arc<RuleSet>) -> SharedMessageQueue {
        Arc::new(Self {
            emitter,
            state: Mutex::new(MessageQueueState {
                pending_messages: VecDeque::new(),
                waiting_for_ack: false,
                rules,
                scheduled: Vec::new(),
                next_seq: 0,
                closed: false,
            }),
            ready: Notify::new(),
        })
    }

    /// Replaces the rules; replies scheduled by the previous rules are dropped.
    pub(crate) async fn update_rules(&self, rules: Arc<RuleSet>) {
        let mut state = self.state.lock().await;
        let dropped = state.scheduled.len();
        state.scheduled.clear();
        state.rules = rules;
        drop(state);
        if dropped > 0 {
            self.emitter.only_log(
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
        state.closed = true;
        state.pending_messages.clear();
        state.scheduled.clear();
    }

    pub(crate) async fn send_user_message(&self, payload: &SendRequest) {
        self.emitter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!(
                "sending user message lines={} chars={}",
                payload.message.lines().count(),
                payload.message.chars().count()
            ),
        );

        self.enqueue_message(&payload.message).await;
    }

    async fn enqueue_message(&self, message: &str) {
        let mut state = self.state.lock().await;
        if state.closed {
            return;
        }
        state
            .pending_messages
            .extend(message.lines().map(translate::to_bytes));
        drop(state);
        self.ready.notify_one();
    }

    pub(crate) async fn recv(&self) -> Vec<u8> {
        loop {
            let ready = self.ready.notified();
            let wake_at;

            {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                if let Some(frame) = state.take_due(now) {
                    self.emitter.only_log(
                        LogLevel::Inf,
                        file!(),
                        line!(),
                        format!("releasing automatic reply bytes={}", frame.len()),
                    );
                    return frame;
                }
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
                wake_at = state.next_wakeup(now);
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

    /// Raw chunk as read from the socket: shown as is, and an ACK releases the user queue.
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
        self.emitter
            .emit_message_with_raw(MessageType::Received, visible, message.clone());
        // Standalone ACKs are detected by the frame parser (`handle_frame`), which also copes
        // with ACKs coalesced with other data.
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
        self.emitter.emit_message_with_raw(
            MessageType::Sent,
            translate::to_human_readable(msg),
            msg.to_vec(),
        );
    }

    /// A complete protocol message (see `frames`): evaluates the rules once and schedules the
    /// reply. Failures are reported with metadata only (never the peer's content).
    pub(crate) async fn handle_frame(&self, event: FrameEvent) {
        // The ACK of our own transmission releases the send queue whatever the rules say.
        if event == FrameEvent::Ack {
            let _ = self.handle_ack().await;
            return;
        }
        let mut state = self.state.lock().await;
        if state.closed || !state.rules.is_enabled() {
            return;
        }
        match &event {
            FrameEvent::Malformed(reason) => {
                drop(state);
                self.emitter.warn(
                    file!(),
                    line!(),
                    format!("Auto reply: {reason}; no reply was sent"),
                );
                return;
            }
            FrameEvent::Overflow => {
                drop(state);
                self.emitter.error(
                    file!(),
                    line!(),
                    "Auto reply: an incoming message is too large and was ignored; no reply was sent",
                );
                return;
            }
            _ => {}
        }

        match state.rules.evaluate(&event) {
            Outcome::NoMatch => {
                drop(state);
                self.emitter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    "no auto reply rule matched",
                );
            }
            Outcome::Suppressed { rule_name } => {
                drop(state);
                self.emitter.only_log(
                    LogLevel::Inf,
                    file!(),
                    line!(),
                    format!("auto reply rule \"{rule_name}\" matched: no auto reply"),
                );
            }
            Outcome::Failed { rule_name, reason } => {
                drop(state);
                self.emitter.error(
                    file!(),
                    line!(),
                    format!("Auto reply rule \"{rule_name}\" could not reply: {reason}"),
                );
            }
            Outcome::Reply(reply) => {
                let due = Instant::now() + reply.delay;
                let frames = reply.frames.len();
                for frame in reply.frames {
                    let seq = state.next_seq;
                    state.next_seq += 1;
                    state.scheduled.push(ScheduledReply { due, seq, frame });
                }
                drop(state);
                self.emitter.only_log(
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

fn requires_ack(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }

    line[0] == ControlToken::ENQ as u8 || line[0] == ControlToken::STX as u8
}

/// A pure acknowledgement (`ACK` / `NAK`): the only automatic reply allowed to bypass the wait
/// for the ACK of our own transmission.
fn is_control_reply(frame: &[u8]) -> bool {
    frame == [ControlToken::ACK as u8] || frame == [ControlToken::NAK as u8]
}
