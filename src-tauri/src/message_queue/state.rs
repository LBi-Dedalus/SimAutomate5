//! Queue state: bounded admission of user items and scheduled replies, and ACK-wait scheduling.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::protocol::{is_control_reply, requires_ack};
use crate::auto_reply::AutoReplyRuleSet;

pub(super) struct ScheduledReply {
    pub(super) due: Instant,
    pub(super) seq: u64,
    pub(super) frame: Vec<u8>,
}

/// Ceiling on queued items (user items + scheduled reply frames, combined).
pub(super) const MAX_ITEMS: usize = 1_024;
/// Ceiling on queued bytes (user items + scheduled reply frames, combined).
pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;

/// How long an ENQ/STX item waits for the peer's ACK before the send queue is reset.
pub(crate) const ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Which queue the ENQ/STX frame whose ACK is awaited came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AckOrigin {
    /// A user message (`pending_messages`).
    User,
    /// An automatic transmission (`scheduled`).
    Scheduled,
}

/// An awaited ACK: where the frame came from and when the ACK becomes overdue.
#[derive(Clone, Copy, Debug)]
pub(super) struct AckWait {
    pub(super) origin: AckOrigin,
    pub(super) deadline: Instant,
}

/// What `reset_sending` discarded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Discarded {
    /// Queued user items.
    user: usize,
    /// Scheduled automatic frames (including pure ACK/NAK replies).
    scheduled: usize,
}

impl Discarded {
    pub(super) fn total(self) -> usize {
        self.user + self.scheduled
    }

    /// Text for the warning, matching exactly what was dropped.
    pub(super) fn describe(self) -> String {
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

pub(super) struct MessageQueueState {
    pub(super) pending_messages: VecDeque<Vec<u8>>,
    /// `Some` while an ACK is awaited, `None` otherwise.
    pub(super) ack_wait: Option<AckWait>,
    pub(super) ack_timeout: Duration,
    pub(super) rules: Arc<AutoReplyRuleSet>,
    pub(super) scheduled: Vec<ScheduledReply>,
    next_seq: u64,
    /// Items in `pending_messages` and `scheduled` combined.
    pub(super) queued_items: usize,
    /// Bytes in `pending_messages` and `scheduled` combined.
    pub(super) queued_bytes: usize,
    /// Set when the attempt ended: nothing is queued or scheduled any more.
    pub(super) closed: bool,
}

impl MessageQueueState {
    pub(super) fn new(rules: Arc<AutoReplyRuleSet>, ack_timeout: Duration) -> Self {
        Self {
            pending_messages: VecDeque::new(),
            ack_wait: None,
            ack_timeout,
            rules,
            scheduled: Vec::new(),
            next_seq: 0,
            queued_items: 0,
            queued_bytes: 0,
            closed: false,
        }
    }

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
    pub(super) fn push_user(&mut self, items: Vec<Vec<u8>>) -> Result<(), String> {
        let bytes = items.iter().map(Vec::len).sum();
        self.check(items.len(), bytes)?;
        self.queued_items += items.len();
        self.queued_bytes += bytes;
        self.pending_messages.extend(items);
        Ok(())
    }

    /// All-or-nothing admission of every frame of one automatic reply.
    pub(super) fn push_replies(
        &mut self,
        due: Instant,
        frames: Vec<Vec<u8>>,
    ) -> Result<(), String> {
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

    pub(super) fn pop_user(&mut self) -> Option<Vec<u8>> {
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
    pub(super) fn clear_replies(&mut self) -> usize {
        let count = self.scheduled.len();
        let bytes: usize = self.scheduled.iter().map(|r| r.frame.len()).sum();
        self.scheduled.clear();
        self.queued_items -= count;
        self.queued_bytes -= bytes;
        count
    }

    /// Whether an ACK is currently awaited.
    pub(super) fn waiting_for_ack(&self) -> bool {
        self.ack_wait.is_some()
    }

    /// Starts (or not, depending on the frame) the wait for the ACK of a frame just released.
    pub(super) fn start_ack_wait(&mut self, frame: &[u8], now: Instant, origin: AckOrigin) {
        self.ack_wait = requires_ack(frame).then(|| AckWait {
            origin,
            deadline: now + self.ack_timeout,
        });
    }

    /// Stops waiting for an ACK.
    pub(super) fn stop_ack_wait(&mut self) {
        self.ack_wait = None;
    }

    /// Whether the awaited ACK is overdue at `now`.
    pub(super) fn ack_overdue(&self, now: Instant) -> bool {
        self.ack_wait.is_some_and(|wait| now >= wait.deadline)
    }

    /// Stops sending: drops the waiting state and what would continue the aborted transmission.
    /// - The awaited frame came from a user message (or nothing was awaited): the queued user
    ///   items are dropped; scheduled automatic frames are kept.
    /// - It came from an automatic transmission: ALL scheduled frames are dropped (the rest of
    ///   that transmission, e.g. its STX/EOT, and also pure `<ACK>`/`<NAK>` replies); queued
    ///   user items are kept.
    ///
    /// Returns what was discarded.
    pub(super) fn reset_sending(&mut self) -> Discarded {
        let origin = self.ack_wait.map(|wait| wait.origin);
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
    pub(super) fn close(&mut self) {
        self.stop_ack_wait();
        self.closed = true;
        self.pending_messages.clear();
        self.scheduled.clear();
        self.queued_items = 0;
        self.queued_bytes = 0;
    }

    /// Earliest due reply that may be released now.
    pub(super) fn take_due(&mut self, now: Instant) -> Option<Vec<u8>> {
        let waiting = self.waiting_for_ack();
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
    pub(super) fn next_wakeup(&self, now: Instant) -> Option<Instant> {
        let waiting = self.waiting_for_ack();
        self.scheduled
            .iter()
            .filter(|reply| reply.due > now && (!waiting || is_control_reply(&reply.frame)))
            .map(|reply| reply.due)
            .min()
    }
}
