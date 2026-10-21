use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

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

/// Ceiling on queued items (user items + scheduled reply frames, combined).
const MAX_ITEMS: usize = 1_024;
/// Ceiling on queued bytes (user items + scheduled reply frames, combined).
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Largest user message accepted by `send_user_message`.
const MAX_USER_PAYLOAD: usize = 1024 * 1024;

/// How long an ENQ/STX item waits for the peer's ACK before the send queue is reset.
pub(crate) const ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Which queue the ENQ/STX frame whose ACK is awaited came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AckOrigin {
    /// A user message (`pending_messages`).
    User,
    /// An automatic transmission (`scheduled`).
    Scheduled,
}

/// What `reset_sending` discarded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Discarded {
    /// Queued user items.
    user: usize,
    /// Scheduled automatic frames (including pure ACK/NAK replies).
    scheduled: usize,
}

impl Discarded {
    fn total(self) -> usize {
        self.user + self.scheduled
    }

    /// Text for the warning, matching exactly what was dropped.
    fn describe(self) -> String {
        if self.scheduled == 0 {
            format!("{} queued item(s) dropped", self.user)
        } else if self.user == 0 {
            format!("{} scheduled automatic frame(s) dropped", self.scheduled)
        } else {
            format!(
                "{} queued item(s) and {} scheduled automatic frame(s) dropped",
                self.user, self.scheduled
            )
        }
    }
}

struct MessageQueueState {
    pending_messages: VecDeque<Vec<u8>>,
    waiting_for_ack: bool,
    /// Origin of the frame being awaited (set together with `waiting_for_ack`).
    ack_origin: Option<AckOrigin>,
    /// When the awaited ACK is overdue (set together with `waiting_for_ack`).
    ack_deadline: Option<Instant>,
    ack_timeout: Duration,
    rules: Arc<RuleSet>,
    scheduled: Vec<ScheduledReply>,
    next_seq: u64,
    /// Items in `pending_messages` and `scheduled` combined.
    queued_items: usize,
    /// Bytes in `pending_messages` and `scheduled` combined.
    queued_bytes: usize,
    /// Set when the attempt ended: nothing is queued or scheduled any more.
    closed: bool,
}

impl MessageQueueState {
    /// Whether `items` more items totalling `bytes` bytes fit.
    fn check(&self, items: usize, bytes: usize) -> Result<(), String> {
        if self.queued_items + items > MAX_ITEMS || self.queued_bytes + bytes > MAX_BYTES {
            return Err(format!(
                "send queue full ({} items, {} bytes)",
                self.queued_items, self.queued_bytes
            ));
        }
        Ok(())
    }

    /// All-or-nothing admission of user items.
    fn push_user(&mut self, items: Vec<Vec<u8>>) -> Result<(), String> {
        let bytes = items.iter().map(Vec::len).sum();
        self.check(items.len(), bytes)?;
        self.queued_items += items.len();
        self.queued_bytes += bytes;
        self.pending_messages.extend(items);
        Ok(())
    }

    /// All-or-nothing admission of every frame of one automatic reply.
    fn push_replies(&mut self, due: Instant, frames: Vec<Vec<u8>>) -> Result<(), String> {
        let bytes = frames.iter().map(Vec::len).sum();
        self.check(frames.len(), bytes)?;
        self.queued_items += frames.len();
        self.queued_bytes += bytes;
        for frame in frames {
            let seq = self.next_seq;
            self.next_seq += 1;
            self.scheduled.push(ScheduledReply { due, seq, frame });
        }
        Ok(())
    }

    fn pop_user(&mut self) -> Option<Vec<u8>> {
        let item = self.pending_messages.pop_front()?;
        self.release(item.len());
        Some(item)
    }

    fn pop_reply(&mut self, index: usize) -> Vec<u8> {
        let frame = self.scheduled.remove(index).frame;
        self.release(frame.len());
        frame
    }

    /// Only called by `pop_*`.
    fn release(&mut self, bytes: usize) {
        self.queued_items -= 1;
        self.queued_bytes -= bytes;
    }

    /// Drops every scheduled reply and returns how many there were.
    fn clear_replies(&mut self) -> usize {
        let count = self.scheduled.len();
        let bytes: usize = self.scheduled.iter().map(|r| r.frame.len()).sum();
        self.scheduled.clear();
        self.queued_items -= count;
        self.queued_bytes -= bytes;
        count
    }

    /// Starts (or not, depending on the frame) the wait for the ACK of a frame just released.
    fn start_ack_wait(&mut self, frame: &[u8], now: Instant, origin: AckOrigin) {
        self.waiting_for_ack = requires_ack(frame);
        self.ack_deadline = self.waiting_for_ack.then(|| now + self.ack_timeout);
        self.ack_origin = self.waiting_for_ack.then_some(origin);
    }

    /// Stops waiting for an ACK.
    fn stop_ack_wait(&mut self) {
        self.waiting_for_ack = false;
        self.ack_deadline = None;
        self.ack_origin = None;
    }

    /// Whether the awaited ACK is overdue at `now`.
    fn ack_overdue(&self, now: Instant) -> bool {
        self.waiting_for_ack && self.ack_deadline.is_some_and(|deadline| now >= deadline)
    }

    /// Stops sending: drops the waiting state and what would continue the aborted transmission.
    /// - The awaited frame came from a user message (or nothing was awaited): the queued user
    ///   items are dropped; scheduled automatic frames are kept.
    /// - It came from an automatic transmission: ALL scheduled frames are dropped (the rest of
    ///   that transmission, e.g. its STX/EOT, and also pure `<ACK>`/`<NAK>` replies); queued
    ///   user items are kept.
    /// Returns what was discarded.
    fn reset_sending(&mut self) -> Discarded {
        let origin = self.ack_origin;
        self.stop_ack_wait();
        let mut discarded = Discarded::default();
        if origin == Some(AckOrigin::Scheduled) {
            discarded.scheduled = self.scheduled.len();
            let bytes: usize = self.scheduled.iter().map(|reply| reply.frame.len()).sum();
            self.scheduled.clear();
            self.queued_items -= discarded.scheduled;
            self.queued_bytes -= bytes;
        } else {
            discarded.user = self.pending_messages.len();
            let bytes: usize = self.pending_messages.iter().map(Vec::len).sum();
            self.pending_messages.clear();
            self.queued_items -= discarded.user;
            self.queued_bytes -= bytes;
        }
        discarded
    }

    /// Ends the attempt: nothing stays queued.
    fn close(&mut self) {
        self.stop_ack_wait();
        self.closed = true;
        self.pending_messages.clear();
        self.scheduled.clear();
        self.queued_items = 0;
        self.queued_bytes = 0;
    }

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
        let frame = self.pop_reply(index);
        if !is_control_reply(&frame) {
            self.start_ack_wait(&frame, now, AckOrigin::Scheduled);
        }
        Some(frame)
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
        Self::with_ack_timeout(emitter, rules, ACK_TIMEOUT)
    }

    /// Same as [`Self::shared`] with a custom ACK wait (tests use short durations).
    pub(crate) fn with_ack_timeout(
        emitter: Emitter,
        rules: Arc<RuleSet>,
        ack_timeout: Duration,
    ) -> SharedMessageQueue {
        Arc::new(Self {
            emitter,
            state: Mutex::new(MessageQueueState {
                pending_messages: VecDeque::new(),
                waiting_for_ack: false,
                ack_origin: None,
                ack_deadline: None,
                ack_timeout,
                rules,
                scheduled: Vec::new(),
                next_seq: 0,
                queued_items: 0,
                queued_bytes: 0,
                closed: false,
            }),
            ready: Notify::new(),
        })
    }

    /// Replaces the rules; replies scheduled by the previous rules are dropped.
    pub(crate) async fn update_rules(&self, rules: Arc<RuleSet>) {
        let mut state = self.state.lock().await;
        let dropped = state.clear_replies();
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
        state.close();
    }

    pub(crate) async fn send_user_message(&self, payload: &SendRequest) -> Result<(), String> {
        if payload.message.len() > MAX_USER_PAYLOAD {
            return Err(format!(
                "Message too large ({} bytes, maximum {MAX_USER_PAYLOAD})",
                payload.message.len()
            ));
        }
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

        self.enqueue_message(&payload.message).await
    }

    async fn enqueue_message(&self, message: &str) -> Result<(), String> {
        let items = split_message(message);
        let mut state = self.state.lock().await;
        if state.closed {
            return Ok(());
        }
        state.push_user(items)?;
        drop(state);
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
                    drop(state);
                    self.report_ack_timeout(dropped, timeout);
                    continue;
                }
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
                    if let Some(message) = state.pop_user() {
                        state.start_ack_wait(&message, now, AckOrigin::User);
                        self.emitter.only_log(
                            LogLevel::Inf,
                            file!(),
                            line!(),
                            format!("releasing queued message bytes={}", message.len()),
                        );
                        return message;
                    }
                }
                let ack_wake = state.ack_deadline.filter(|_| state.waiting_for_ack);
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

    /// Raw chunk as read from the socket: shown as is, and an ACK releases the user queue.
    /// `visible` is the text decoded by the connection's stream decoder (so characters split
    /// across reads are reassembled); `message` is exactly the bytes of this read.
    pub(crate) async fn handle_received_message(
        &self,
        visible: String,
        message: Vec<u8>,
    ) -> Result<(), String> {
        if message.is_empty() {
            return Err("Connection closed by peer".to_string());
        }

        self.emitter.only_log(
            LogLevel::Inf,
            file!(),
            line!(),
            format!("received message bytes={}", message.len()),
        );

        self.emitter
            .emit_message_with_raw(MessageType::Received, visible, message.clone());
        // Standalone ACKs are detected by the frame parser (`handle_frame`), which also copes
        // with ACKs coalesced with other data.
        Ok(())
    }

    /// Logs (metadata only) bytes of an incomplete UTF-8 sequence dropped when the peer closed.
    pub(crate) fn log_incomplete_on_close(&self, pending_bytes: usize) {
        if pending_bytes > 0 {
            self.emitter.only_log(
                LogLevel::Inf,
                file!(),
                line!(),
                format!("connection closed with incomplete UTF-8 sequence bytes={pending_bytes}"),
            );
        }
    }

    fn report_ack_timeout(&self, dropped: Discarded, timeout: Duration) {
        self.emitter.warn(
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
            let was_waiting = state.waiting_for_ack;
            let dropped = state.reset_sending();
            (was_waiting, dropped)
        };
        match outcome {
            (false, dropped) if dropped.total() == 0 => self.emitter.only_log(
                LogLevel::Inf,
                file!(),
                line!(),
                format!("received {what} with nothing to send"),
            ),
            (_, dropped) => self.emitter.warn(
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
            } else if state.waiting_for_ack {
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
                let pushed = state.push_replies(due, reply.frames);
                drop(state);
                if let Err(e) = pushed {
                    self.emitter.error(
                        file!(),
                        line!(),
                        format!("Auto reply rule \"{}\" dropped: {e}", reply.rule_name),
                    );
                    return;
                }
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

/// Position of the first of `needles` in `text` (with the needle length).
fn find_first(text: &str, needles: &[&str]) -> Option<(usize, usize)> {
    needles
        .iter()
        .filter_map(|n| text.find(n).map(|i| (i, n.len())))
        .min_by_key(|(i, _)| *i)
}

fn starts_mllp(text: &str) -> bool {
    text.starts_with("<VT>") || text.starts_with('\u{0B}')
}

/// Translates `text` line by line (lines as `str::lines()` sees them: a CR is dropped only
/// when it precedes the LF that ends the line) and concatenates the bytes, so that the line
/// breaks are not sent and a token can never span two lines.
fn translate_lines_joined(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (line, next) = match rest.find('\n') {
            Some(index) => {
                let line = &rest[..index];
                (line.strip_suffix('\r').unwrap_or(line), &rest[index + 1..])
            }
            None => (rest, ""),
        };
        bytes.extend(translate::to_bytes(line));
        rest = next;
    }
    bytes
}

/// Splits the composer text into the items written to the socket.
///
/// An HL7/MLLP frame (`<VT>` ... `<FS>` plus the following `<CR>`, human-readable tokens or
/// raw bytes) is ONE item: the line breaks inside it are composer formatting and are not sent.
/// Leading whitespace before `<VT>` is skipped; an unterminated frame runs to the end of the
/// text. Everything else keeps the line-based behaviour (one item per line, newlines dropped),
/// which the ASTM ENQ/STX/ACK handling relies on.
fn split_message(message: &str) -> Vec<Vec<u8>> {
    let mut items = Vec::new();
    let mut rest = message;
    while !rest.is_empty() {
        let trimmed = rest.trim_start_matches(['\n', '\r', ' ', '\t']);
        if starts_mllp(trimmed) {
            let end = match find_first(trimmed, &["<FS>", "\u{1C}"]) {
                Some((index, len)) => {
                    let mut end = index + len;
                    let after = &trimmed[end..];
                    if after.starts_with("<CR>") {
                        end += 4;
                    } else if after.starts_with('\r') && !after.starts_with("\r\n") {
                        end += 1;
                    }
                    end
                }
                None => trimmed.len(),
            };
            items.push(translate_lines_joined(&trimmed[..end]));
            rest = &trimmed[end..];
            // The line break that ends the frame's last line is formatting too.
            rest = rest
                .strip_prefix("\r\n")
                .or_else(|| rest.strip_prefix('\n'))
                .unwrap_or(rest);
        } else {
            // A CR is stripped only when it precedes the LF consumed here; a trailing CR
            // without LF is data (as with `str::lines()`).
            let (line, next) = match rest.find('\n') {
                Some(index) => {
                    let line = &rest[..index];
                    (line.strip_suffix('\r').unwrap_or(line), &rest[index + 1..])
                }
                None => (rest, ""),
            };
            items.push(translate::to_bytes(line));
            rest = next;
        }
    }
    items
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::timeout;

    use super::*;
    use crate::models::{MessagePayload, StatusPayload};

    struct NoSink;

    impl crate::emitter::EventSink for NoSink {
        fn status(&self, _: StatusPayload) -> Result<(), String> {
            Ok(())
        }
        fn message(&self, _: MessagePayload) -> Result<(), String> {
            Ok(())
        }
        fn notify(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn queue() -> SharedMessageQueue {
        MessageQueue::shared(
            Emitter::with_sink(Arc::new(NoSink)),
            Arc::new(RuleSet::disabled()),
        )
    }

    const AUTOBUILD: &str = "<VT>\nMSH|^~\\&|A|B<CR>\nPID|1||123<CR>\n<FS><CR>";
    const AUTOBUILD_BYTES: &[u8] = b"\x0bMSH|^~\\&|A|B\rPID|1||123\r\x1c\r";

    struct CaptureSink(std::sync::Mutex<Vec<(String, Vec<u8>)>>);

    impl crate::emitter::EventSink for CaptureSink {
        fn status(&self, _: StatusPayload) -> Result<(), String> {
            Ok(())
        }
        fn message(&self, p: MessagePayload) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((p.content, p.raw.unwrap_or_default()));
            Ok(())
        }
        fn notify(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn split_multibyte_char_is_reassembled_and_raw_is_per_read() {
        let sink = Arc::new(CaptureSink(Default::default()));
        let q = MessageQueue::shared(
            Emitter::with_sink(sink.clone()).scoped(
                &crate::ids::SessionId::parse("s").unwrap(),
                crate::ids::Attempt::parse(1).unwrap(),
            ),
            Arc::new(RuleSet::disabled()),
        );
        let mut dec = translate::Utf8StreamDecoder::new();
        for chunk in [&[0x43u8, 0x61, 0x66, 0xC3][..], &[0xA9][..]] {
            let visible = dec.decode(chunk);
            q.handle_received_message(visible, chunk.to_vec())
                .await
                .unwrap();
        }
        let got = sink.0.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![
                ("Caf".to_string(), vec![0x43, 0x61, 0x66, 0xC3]),
                ("é".to_string(), vec![0xA9]),
            ]
        );
    }

    #[test]
    fn autobuild_hl7_is_one_item_with_exact_bytes() {
        assert_eq!(split_message(AUTOBUILD), vec![AUTOBUILD_BYTES.to_vec()]);
    }

    #[test]
    fn hl7_tolerates_crlf_trailing_newline_and_leading_whitespace() {
        let crlf = format!("\r\n  {}\r\n", AUTOBUILD.replace('\n', "\r\n"));
        assert_eq!(split_message(&crlf), vec![AUTOBUILD_BYTES.to_vec()]);
        let lf = format!("\n\n{AUTOBUILD}\n");
        assert_eq!(split_message(&lf), vec![AUTOBUILD_BYTES.to_vec()]);
    }

    #[test]
    fn hl7_with_raw_control_bytes_is_one_item() {
        let text = "\u{0B}MSH|A\r\nPID|1\r\u{1C}\r";
        assert_eq!(
            split_message(text),
            vec![b"\x0bMSH|APID|1\r\x1c\r".to_vec()]
        );
    }

    #[test]
    fn two_frames_are_two_items() {
        let text = format!("{AUTOBUILD}\n{AUTOBUILD}\n");
        assert_eq!(
            split_message(&text),
            vec![AUTOBUILD_BYTES.to_vec(), AUTOBUILD_BYTES.to_vec()]
        );
    }

    #[test]
    fn unterminated_frame_is_sent_as_one_item() {
        assert_eq!(
            split_message("<VT>\nMSH|A<CR>\nPID|1<CR>\n"),
            vec![b"\x0bMSH|A\rPID|1\r".to_vec()]
        );
    }

    #[test]
    fn frame_without_trailing_cr_ends_at_fs() {
        assert_eq!(
            split_message("<VT>MSH|A<CR><FS>\nplain"),
            vec![b"\x0bMSH|A\r\x1c".to_vec(), b"plain".to_vec()]
        );
    }

    #[test]
    fn text_outside_frames_stays_line_based() {
        assert_eq!(
            split_message(&format!("one\r\ntwo\n\n  {AUTOBUILD}\nthree")),
            vec![
                b"one".to_vec(),
                b"two".to_vec(),
                AUTOBUILD_BYTES.to_vec(),
                b"three".to_vec()
            ]
        );
    }

    #[test]
    fn astm_and_plain_text_stay_one_item_per_line() {
        assert_eq!(
            split_message("<ENQ>\n<STX>1H|\\^&<CR><ETX>00<CR><LF>\n<EOT>"),
            vec![vec![0x05], b"\x021H|\\^&\r\x0300\r\n".to_vec(), vec![0x04]]
        );
        assert_eq!(split_message("a\nb\n"), vec![b"a".to_vec(), b"b".to_vec()]);
        assert!(split_message("").is_empty());
    }

    #[test]
    fn tokens_split_across_lines_are_not_joined_into_tokens() {
        let expected = vec![b"\x0bMSH|A<CR>\r\x1c\r".to_vec()];
        assert_eq!(split_message("<VT>MSH|A<C\nR><CR><FS><CR>"), expected);
        assert_eq!(split_message("<VT>MSH|A<C\r\nR><CR><FS><CR>"), expected);
    }

    #[test]
    fn lone_terminal_cr_is_data() {
        assert_eq!(split_message("plain\r"), vec![b"plain\r".to_vec()]);
        assert_eq!(split_message("\r"), vec![vec![0x0d]]);
        assert_eq!(split_message("a\r\n\r"), vec![b"a".to_vec(), vec![0x0d]]);
        assert_eq!(
            split_message("<STX>1H|<CR>\r"),
            vec![b"\x021H|\r\r".to_vec()]
        );
        assert_eq!(split_message("<ENQ>\r"), vec![vec![0x05, 0x0d]]);
    }

    #[test]
    fn non_hl7_text_matches_per_line_translation() {
        let atoms = [
            "a", "<CR>", "<C", "R>", "\r", "\n", "\r\n", " ", "<ENQ>", "x|y",
        ];
        let mut seed = 12345u32;
        for _ in 0..500 {
            let mut text = String::new();
            for _ in 0..8 {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                text.push_str(atoms[(seed >> 16) as usize % atoms.len()]);
            }
            let old: Vec<Vec<u8>> = text.lines().map(translate::to_bytes).collect();
            assert_eq!(split_message(&text), old, "{text:?}");
        }
    }

    #[tokio::test]
    async fn hl7_frame_is_released_in_one_piece_without_waiting_for_ack() {
        let queue = queue();
        queue
            .enqueue_message(&format!("{AUTOBUILD}\n{AUTOBUILD}"))
            .await
            .unwrap();
        for _ in 0..2 {
            let item = timeout(Duration::from_millis(200), queue.recv())
                .await
                .expect("frame released");
            assert_eq!(item, AUTOBUILD_BYTES);
        }
    }

    #[tokio::test]
    async fn astm_items_still_wait_for_ack() {
        let queue = queue();
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        assert!(timeout(Duration::from_millis(100), queue.recv())
            .await
            .is_err());
        queue.handle_frame(FrameEvent::Ack).await;
        assert_eq!(queue.recv().await, b"second".to_vec());
    }

    struct TypedSink(std::sync::Mutex<Vec<(String, String)>>);

    impl crate::emitter::EventSink for TypedSink {
        fn status(&self, _: StatusPayload) -> Result<(), String> {
            Ok(())
        }
        fn message(&self, p: MessagePayload) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((format!("{:?}", p.msg_type), p.content));
            Ok(())
        }
        fn notify(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    const SHORT: Duration = Duration::from_millis(60);

    fn short_queue(rules: Arc<RuleSet>) -> (SharedMessageQueue, Arc<TypedSink>) {
        let sink = Arc::new(TypedSink(Default::default()));
        let q = MessageQueue::with_ack_timeout(
            Emitter::with_sink(sink.clone()).scoped(
                &crate::ids::SessionId::parse("s").unwrap(),
                crate::ids::Attempt::parse(1).unwrap(),
            ),
            rules,
            SHORT,
        );
        (q, sink)
    }

    fn warnings(sink: &TypedSink) -> Vec<String> {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(kind, _)| kind == "SystemWarn")
            .map(|(_, content)| content.clone())
            .collect()
    }

    async fn assert_idle(queue: &SharedMessageQueue) {
        let state = queue.state.lock().await;
        assert!(!state.waiting_for_ack);
        assert!(state.ack_deadline.is_none());
        assert_eq!((state.queued_items, state.queued_bytes), (0, 0));
        assert_invariant(&state);
    }

    #[test]
    fn default_ack_timeout_is_ten_seconds() {
        assert_eq!(ACK_TIMEOUT, Duration::from_secs(10));
        assert_eq!(state().ack_timeout, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn missing_ack_times_out_clears_queue_and_reports() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond\nthird").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        // Nothing else is released: the queue was cleared when the ACK window elapsed.
        assert!(timeout(Duration::from_millis(400), queue.recv())
            .await
            .is_err());
        assert_idle(&queue).await;
        let warns = warnings(&sink);
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert!(warns[0].contains("No ACK received within 60 ms"));
        // The queue is usable again.
        queue.enqueue_message("next").await.unwrap();
        assert_eq!(queue.recv().await, b"next".to_vec());
    }

    #[tokio::test]
    async fn ack_within_the_window_does_not_time_out() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        queue.handle_frame(FrameEvent::Ack).await;
        assert_eq!(queue.recv().await, b"second".to_vec());
        tokio::time::sleep(SHORT * 2).await;
        assert!(warnings(&sink).is_empty());
    }

    #[tokio::test]
    async fn late_ack_after_reset_is_ignored() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        assert!(timeout(Duration::from_millis(300), queue.recv())
            .await
            .is_err());
        queue.handle_frame(FrameEvent::Ack).await;
        assert_idle(&queue).await;
        assert_eq!(warnings(&sink).len(), 1);
        assert!(timeout(Duration::from_millis(100), queue.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn ack_arriving_after_the_deadline_is_a_timeout_not_a_release() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        tokio::time::sleep(SHORT * 2).await; // recv is not polled: nobody noticed yet
        queue.handle_frame(FrameEvent::Ack).await;
        assert_idle(&queue).await;
        assert_eq!(warnings(&sink).len(), 1);
    }

    #[tokio::test]
    async fn duplicate_ack_is_harmless() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        queue.handle_frame(FrameEvent::Ack).await;
        assert_eq!(queue.recv().await, b"second".to_vec());
        queue.handle_frame(FrameEvent::Ack).await; // duplicate
        assert_idle(&queue).await;
        assert!(warnings(&sink).is_empty());
        queue.enqueue_message("third").await.unwrap();
        assert_eq!(queue.recv().await, b"third".to_vec());
    }

    #[tokio::test]
    async fn nak_stops_sending_and_clears_the_queue_without_resend() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond\nthird").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        queue.handle_frame(FrameEvent::Nak).await;
        assert_idle(&queue).await;
        let warns = warnings(&sink);
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("NAK"));
        assert!(timeout(Duration::from_millis(200), queue.recv())
            .await
            .is_err());
        // Not blocked any more, and the timeout does not fire later.
        queue.enqueue_message("next").await.unwrap();
        assert_eq!(queue.recv().await, b"next".to_vec());
        assert_eq!(warnings(&sink).len(), 1);
    }

    #[tokio::test]
    async fn eot_does_not_leave_the_queue_blocked() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        queue.handle_frame(FrameEvent::Eot).await;
        assert_idle(&queue).await;
        assert!(warnings(&sink)[0].contains("EOT"));
        // Idle NAK/EOT are only logged.
        queue.handle_frame(FrameEvent::Nak).await;
        queue.handle_frame(FrameEvent::Eot).await;
        assert_eq!(warnings(&sink).len(), 1);
    }

    #[tokio::test]
    async fn disconnect_cancels_the_ack_wait() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        queue.close().await;
        assert_idle(&queue).await;
        tokio::time::sleep(SHORT * 3).await;
        assert!(warnings(&sink).is_empty());
        queue.handle_frame(FrameEvent::Ack).await;
        queue.handle_frame(FrameEvent::Nak).await;
        assert!(warnings(&sink).is_empty());
    }

    #[tokio::test]
    async fn pending_recv_is_woken_by_the_deadline_and_stays_usable() {
        let (queue, _sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        let waiter = tokio::spawn({
            let queue = queue.clone();
            async move { queue.recv().await }
        });
        tokio::time::sleep(SHORT * 3).await;
        assert!(!waiter.is_finished());
        queue.enqueue_message("after").await.unwrap();
        assert_eq!(waiter.await.unwrap(), b"after".to_vec());
    }

    #[tokio::test]
    async fn control_reply_still_goes_out_while_waiting_for_our_ack() {
        let (queue, sink) = short_queue(ack_rules());
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        // The peer sends its own ENQ before answering ours: our <ACK> must not wait.
        queue.handle_frame(FrameEvent::Enq).await;
        assert_eq!(queue.recv().await, vec![0x06]);
        assert!(queue.state.lock().await.waiting_for_ack);
        // Then the peer ACKs our ENQ: the queue resumes.
        queue.handle_frame(FrameEvent::Ack).await;
        assert_eq!(queue.recv().await, b"second".to_vec());
        assert!(warnings(&sink).is_empty());
    }

    #[tokio::test]
    async fn control_reply_keeps_working_after_a_timeout_reset() {
        let (queue, _sink) = short_queue(ack_rules());
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        assert!(timeout(Duration::from_millis(300), queue.recv())
            .await
            .is_err());
        queue.handle_frame(FrameEvent::Enq).await;
        assert_eq!(queue.recv().await, vec![0x06]);
    }

    const AUTO_STX: &[u8] = b"\x021H|X\r\x035D\r\n";

    /// Schedules an automatic ENQ / STX / EOT transmission and releases its ENQ.
    async fn start_auto_send(queue: &SharedMessageQueue) {
        queue
            .state
            .lock()
            .await
            .push_replies(
                Instant::now(),
                vec![vec![0x05], AUTO_STX.to_vec(), vec![0x04]],
            )
            .unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        let s = queue.state.lock().await;
        assert_eq!(s.ack_origin, Some(AckOrigin::Scheduled));
        assert_eq!(s.scheduled.len(), 2);
    }

    async fn assert_auto_send_aborted(queue: &SharedMessageQueue, sink: &TypedSink, what: &str) {
        // No continuation bytes are released.
        assert!(timeout(Duration::from_millis(200), queue.recv())
            .await
            .is_err());
        assert_idle(queue).await;
        let warns = warnings(sink);
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert!(warns[0].contains(what), "{warns:?}");
        assert!(
            warns[0].contains("2 scheduled automatic frame(s) dropped"),
            "{warns:?}"
        );
        // A fresh transaction proceeds.
        queue
            .state
            .lock()
            .await
            .push_replies(Instant::now(), vec![vec![0x05], AUTO_STX.to_vec()])
            .unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        queue.handle_frame(FrameEvent::Ack).await;
        assert_eq!(queue.recv().await, AUTO_STX.to_vec());
    }

    #[tokio::test]
    async fn automatic_send_timeout_drops_the_continuation() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        start_auto_send(&queue).await;
        assert_auto_send_aborted(&queue, &sink, "No ACK received within 60 ms").await;
    }

    #[tokio::test]
    async fn automatic_send_nak_drops_the_continuation() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        start_auto_send(&queue).await;
        queue.handle_frame(FrameEvent::Nak).await;
        assert_auto_send_aborted(&queue, &sink, "NAK").await;
    }

    #[tokio::test]
    async fn automatic_send_eot_drops_the_continuation() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        start_auto_send(&queue).await;
        queue.handle_frame(FrameEvent::Eot).await;
        assert_auto_send_aborted(&queue, &sink, "EOT").await;
    }

    #[tokio::test]
    async fn aborted_automatic_send_drops_control_replies_and_keeps_user_items() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        start_auto_send(&queue).await;
        queue.enqueue_message("user").await.unwrap();
        queue
            .state
            .lock()
            .await
            .push_replies(Instant::now(), vec![vec![0x06], vec![0x15]])
            .unwrap();
        queue.handle_frame(FrameEvent::Nak).await;
        {
            let s = queue.state.lock().await;
            assert_eq!(s.scheduled.len(), 0);
            assert_eq!(s.pending_messages.len(), 1);
            assert_invariant(&s);
        }
        let warns = warnings(&sink);
        assert!(warns[0].contains("4 scheduled automatic frame(s) dropped"));
        assert!(!warns[0].contains("queued item(s)"));
        assert_eq!(queue.recv().await, b"user".to_vec());
    }

    #[tokio::test]
    async fn aborted_user_send_keeps_scheduled_frames() {
        let (queue, sink) = short_queue(Arc::new(RuleSet::disabled()));
        queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
        assert_eq!(queue.recv().await, vec![0x05]);
        assert_eq!(queue.state.lock().await.ack_origin, Some(AckOrigin::User));
        queue
            .state
            .lock()
            .await
            .push_replies(far(), vec![AUTO_STX.to_vec()])
            .unwrap();
        queue.handle_frame(FrameEvent::Nak).await;
        let s = queue.state.lock().await;
        assert_eq!((s.pending_messages.len(), s.scheduled.len()), (0, 1));
        assert!(s.ack_origin.is_none());
        assert_invariant(&s);
        assert!(warnings(&sink)[0].contains("1 queued item(s) dropped"));
    }

    fn state() -> MessageQueueState {
        MessageQueueState {
            pending_messages: VecDeque::new(),
            waiting_for_ack: false,
            ack_origin: None,
            ack_deadline: None,
            ack_timeout: ACK_TIMEOUT,
            rules: Arc::new(RuleSet::disabled()),
            scheduled: Vec::new(),
            next_seq: 0,
            queued_items: 0,
            queued_bytes: 0,
            closed: false,
        }
    }

    fn assert_invariant(s: &MessageQueueState) {
        let bytes = s.pending_messages.iter().map(Vec::len).sum::<usize>()
            + s.scheduled.iter().map(|r| r.frame.len()).sum::<usize>();
        assert_eq!(s.queued_bytes, bytes);
        assert_eq!(s.queued_items, s.pending_messages.len() + s.scheduled.len());
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(3600)
    }

    fn ack_rules() -> Arc<RuleSet> {
        use crate::auto_reply::{Action, AutoReplyConfig, Rule, Trigger};
        Arc::new(
            RuleSet::compile(
                &AutoReplyConfig {
                    enabled: true,
                    rules: vec![Rule {
                        id: "1".into(),
                        name: "ack".into(),
                        enabled: true,
                        trigger: Trigger::AstmEnq,
                        condition: None,
                        action: Action::Literal {
                            text: "<ACK>".into(),
                        },
                        delay_ms: 0,
                    }],
                },
                &[],
            )
            .unwrap(),
        )
    }

    #[test]
    fn exact_limit_is_accepted() {
        let mut s = state();
        s.push_user(vec![vec![1]; MAX_ITEMS]).unwrap();
        assert_eq!((s.queued_items, s.queued_bytes), (MAX_ITEMS, MAX_ITEMS));
        assert_invariant(&s);

        let mut s = state();
        s.push_user(vec![vec![0; MAX_BYTES / 4]; 4]).unwrap();
        assert_eq!(s.queued_bytes, MAX_BYTES);
        assert_invariant(&s);
    }

    #[test]
    fn one_over_limit_is_rejected() {
        let mut s = state();
        s.push_user(vec![vec![1]; MAX_ITEMS]).unwrap();
        assert!(s.push_user(vec![vec![1]]).is_err());
        assert!(s.push_replies(far(), vec![vec![1]]).is_err());
        assert_eq!((s.queued_items, s.queued_bytes), (MAX_ITEMS, MAX_ITEMS));
        assert_invariant(&s);

        let mut s = state();
        s.push_user(vec![vec![0; MAX_BYTES - 1]]).unwrap();
        assert!(s.push_replies(far(), vec![vec![0; 2]]).is_err());
        assert_eq!((s.queued_items, s.queued_bytes), (1, MAX_BYTES - 1));
        assert!(s.scheduled.is_empty());
        assert_invariant(&s);
    }

    #[test]
    fn rejection_is_all_or_nothing() {
        let mut s = state();
        s.push_user(vec![vec![1]; MAX_ITEMS - 1]).unwrap();
        assert!(s.push_user(vec![vec![1]; 2]).is_err());
        assert!(s.push_replies(far(), vec![vec![1]; 2]).is_err());
        assert_eq!(s.queued_items, MAX_ITEMS - 1);
        assert!(s.scheduled.is_empty());
        assert_invariant(&s);
    }

    #[tokio::test]
    async fn user_payload_too_large_is_rejected() {
        let queue = queue();
        let err = queue
            .send_user_message(&SendRequest {
                message: "a".repeat(MAX_USER_PAYLOAD + 1),
            })
            .await
            .unwrap_err();
        assert!(err.contains("Message too large"));
        let state = queue.state.lock().await;
        assert_eq!((state.queued_items, state.queued_bytes), (0, 0));
        drop(state);
        queue
            .send_user_message(&SendRequest {
                message: "a".repeat(MAX_USER_PAYLOAD),
            })
            .await
            .unwrap();
        assert_eq!(queue.state.lock().await.queued_items, 1);
    }

    #[tokio::test]
    async fn full_queue_rejects_user_message_and_drops_auto_reply() {
        let queue = MessageQueue::shared(Emitter::with_sink(Arc::new(NoSink)), ack_rules());
        // Sanity: with room, the ACK reply is scheduled.
        queue.handle_frame(FrameEvent::Enq).await;
        assert_eq!(queue.state.lock().await.scheduled.len(), 1);
        queue.update_rules(ack_rules()).await;
        assert_eq!(queue.state.lock().await.queued_items, 0);

        queue
            .state
            .lock()
            .await
            .push_user(vec![vec![1]; MAX_ITEMS])
            .unwrap();
        queue.handle_frame(FrameEvent::Enq).await; // pure <ACK> is dropped too
        let err = queue.enqueue_message("x").await.unwrap_err();
        assert!(err.contains("send queue full"));
        let state = queue.state.lock().await;
        assert!(state.scheduled.is_empty());
        assert_eq!(state.queued_items, MAX_ITEMS);
        assert_invariant(&state);
    }

    #[test]
    fn delayed_burst_is_capped() {
        let mut s = state();
        let due = far();
        for _ in 0..10_000 {
            let _ = s.push_replies(due, vec![vec![7; 10]]);
        }
        assert_eq!(s.queued_items, MAX_ITEMS);
        assert!(s.queued_bytes <= MAX_BYTES);
        assert_invariant(&s);
    }

    #[tokio::test]
    async fn drain_resets_counters() {
        let queue = queue();
        let items = MAX_ITEMS / 2;
        queue
            .state
            .lock()
            .await
            .push_user(vec![vec![1, 2, 3]; items])
            .unwrap();
        queue
            .state
            .lock()
            .await
            .push_replies(Instant::now(), vec![vec![9]; 5])
            .unwrap();
        for _ in 0..items + 5 {
            timeout(Duration::from_millis(200), queue.recv())
                .await
                .expect("released");
        }
        let state = queue.state.lock().await;
        assert_eq!((state.queued_items, state.queued_bytes), (0, 0));
        assert_invariant(&state);
    }

    #[tokio::test]
    async fn close_and_update_rules_reset_counters() {
        let queue = queue();
        {
            let mut s = queue.state.lock().await;
            s.push_user(vec![vec![1; 4]; 3]).unwrap();
            s.push_replies(far(), vec![vec![2; 5]; 2]).unwrap();
            assert_eq!((s.queued_items, s.queued_bytes), (5, 22));
            assert_invariant(&s);
        }
        queue.update_rules(Arc::new(RuleSet::disabled())).await;
        {
            let s = queue.state.lock().await;
            assert_eq!((s.queued_items, s.queued_bytes), (3, 12));
            assert_invariant(&s);
        }
        queue
            .state
            .lock()
            .await
            .push_replies(far(), vec![vec![2; 5]])
            .unwrap();
        queue.close().await;
        let s = queue.state.lock().await;
        assert_eq!((s.queued_items, s.queued_bytes), (0, 0));
        assert!(s.closed && s.pending_messages.is_empty() && s.scheduled.is_empty());
    }
}
