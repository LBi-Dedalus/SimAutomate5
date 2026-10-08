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
        state.pending_messages.extend(split_message(message));
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
            .await;
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
        queue.enqueue_message("<ENQ>\nsecond").await;
        assert_eq!(queue.recv().await, vec![0x05]);
        assert!(timeout(Duration::from_millis(100), queue.recv())
            .await
            .is_err());
        queue.handle_frame(FrameEvent::Ack).await;
        assert_eq!(queue.recv().await, b"second".to_vec());
    }
}
