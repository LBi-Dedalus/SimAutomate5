use std::sync::Arc;
use std::time::Duration;

use tokio::time::{timeout, Instant};

use super::composer::split_message;
use super::state::{AckOrigin, MessageQueueState, ACK_TIMEOUT, MAX_BYTES, MAX_ITEMS};
use super::{MessageOrchestrator, SharedMessageOrchestrator, MAX_USER_PAYLOAD};
use crate::auto_reply::AutoReplyRuleSet;
use crate::message_orchestrator::frames::FrameEvent;
use crate::ids::ConnectionId;
use crate::models::{MessagePayload, SendRequest, StatusPayload};
use crate::reporting::NullLogger;
use crate::reporting::{ConnectionReporter, ConnectionReporterFactory};
use crate::translate;

struct NoSink;

fn reporter(sink: Arc<dyn crate::reporting::EventSink>) -> ConnectionReporter {
    ConnectionReporterFactory::new(sink, Arc::new(NullLogger))
        .create(&ConnectionId::parse("s", 1).unwrap())
}

impl crate::reporting::EventSink for NoSink {
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

fn queue() -> SharedMessageOrchestrator {
    MessageOrchestrator::shared(
        reporter(Arc::new(NoSink)),
        Arc::new(AutoReplyRuleSet::disabled()),
    )
}

const AUTOBUILD: &str = "<VT>\nMSH|^~\\&|A|B<CR>\nPID|1||123<CR>\n<FS><CR>";
const AUTOBUILD_BYTES: &[u8] = b"\x0bMSH|^~\\&|A|B\rPID|1||123\r\x1c\r";

struct CaptureSink(std::sync::Mutex<Vec<(String, Vec<u8>)>>);

impl crate::reporting::EventSink for CaptureSink {
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
    let q = MessageOrchestrator::shared(
        reporter(sink.clone()),
        Arc::new(AutoReplyRuleSet::disabled()),
    );
    for chunk in [&[0x43u8, 0x61, 0x66, 0xC3][..], &[0xA9][..]] {
        q.on_received(chunk).await.unwrap();
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
        let old: Vec<Vec<u8>> = text
            .lines()
            .map(translate::ControlTokenEncoder::encode_all)
            .collect();
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

impl crate::reporting::EventSink for TypedSink {
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

fn short_queue(rules: Arc<AutoReplyRuleSet>) -> (SharedMessageOrchestrator, Arc<TypedSink>) {
    let sink = Arc::new(TypedSink(Default::default()));
    let q = MessageOrchestrator::with_ack_timeout(reporter(sink.clone()), rules, SHORT);
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

async fn assert_idle(queue: &SharedMessageOrchestrator) {
    let state = queue.state.lock().await;
    assert!(state.ack_wait.is_none());
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
    queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
    assert_eq!(queue.recv().await, vec![0x05]);
    queue.handle_frame(FrameEvent::Ack).await;
    assert_eq!(queue.recv().await, b"second".to_vec());
    tokio::time::sleep(SHORT * 2).await;
    assert!(warnings(&sink).is_empty());
}

#[tokio::test]
async fn late_ack_after_reset_is_ignored() {
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
    queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
    assert_eq!(queue.recv().await, vec![0x05]);
    tokio::time::sleep(SHORT * 2).await; // recv is not polled: nobody noticed yet
    queue.handle_frame(FrameEvent::Ack).await;
    assert_idle(&queue).await;
    assert_eq!(warnings(&sink).len(), 1);
}

#[tokio::test]
async fn duplicate_ack_is_harmless() {
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    let (queue, _sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    assert!(queue.state.lock().await.ack_wait.is_some());
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
async fn start_auto_send(queue: &SharedMessageOrchestrator) {
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
    assert_eq!(
        s.ack_wait.map(|wait| wait.origin),
        Some(AckOrigin::Scheduled)
    );
    assert_eq!(s.scheduled.len(), 2);
}

async fn assert_auto_send_aborted(queue: &SharedMessageOrchestrator, sink: &TypedSink, what: &str) {
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
    start_auto_send(&queue).await;
    assert_auto_send_aborted(&queue, &sink, "No ACK received within 60 ms").await;
}

#[tokio::test]
async fn automatic_send_nak_drops_the_continuation() {
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
    start_auto_send(&queue).await;
    queue.handle_frame(FrameEvent::Nak).await;
    assert_auto_send_aborted(&queue, &sink, "NAK").await;
}

#[tokio::test]
async fn automatic_send_eot_drops_the_continuation() {
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
    start_auto_send(&queue).await;
    queue.handle_frame(FrameEvent::Eot).await;
    assert_auto_send_aborted(&queue, &sink, "EOT").await;
}

#[tokio::test]
async fn aborted_automatic_send_drops_control_replies_and_keeps_user_items() {
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
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
    let (queue, sink) = short_queue(Arc::new(AutoReplyRuleSet::disabled()));
    queue.enqueue_message("<ENQ>\nsecond").await.unwrap();
    assert_eq!(queue.recv().await, vec![0x05]);
    assert_eq!(
        queue.state.lock().await.ack_wait.map(|wait| wait.origin),
        Some(AckOrigin::User)
    );
    queue
        .state
        .lock()
        .await
        .push_replies(far(), vec![AUTO_STX.to_vec()])
        .unwrap();
    queue.handle_frame(FrameEvent::Nak).await;
    let s = queue.state.lock().await;
    assert_eq!((s.pending_messages.len(), s.scheduled.len()), (0, 1));
    assert!(s.ack_wait.is_none());
    assert_invariant(&s);
    assert!(warnings(&sink)[0].contains("1 queued item(s) dropped"));
}

fn state() -> MessageQueueState {
    MessageQueueState::new(Arc::new(AutoReplyRuleSet::disabled()), ACK_TIMEOUT)
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

fn ack_rules() -> Arc<AutoReplyRuleSet> {
    use crate::auto_reply::{
        AutoReplyAction, AutoReplyTrigger, RawAutoReplyConfig, RawAutoReplyRule,
    };
    Arc::new(
        AutoReplyRuleSet::compile(
            &RawAutoReplyConfig {
                enabled: true,
                rules: vec![RawAutoReplyRule {
                    id: "1".into(),
                    name: "ack".into(),
                    enabled: true,
                    trigger: AutoReplyTrigger::AstmEnq,
                    condition: None,
                    action: AutoReplyAction::Literal {
                        text: "<ACK>".into(),
                    },
                    delay_ms: 0,
                }],
            }
            .parse()
            .unwrap(),
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
    let queue = MessageOrchestrator::shared(reporter(Arc::new(NoSink)), ack_rules());
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
    queue
        .update_rules(Arc::new(AutoReplyRuleSet::disabled()))
        .await;
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
