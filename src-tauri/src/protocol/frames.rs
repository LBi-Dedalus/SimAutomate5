//! Incremental extraction of complete protocol messages from a raw TCP byte stream.
//!
//! The UI keeps showing the raw chunks as they arrive; automatic replies must only react to
//! COMPLETE messages, whatever the way TCP split or coalesced them:
//!
//! * HL7 / MLLP: `VT` ... `FS` `CR`. The event carries the bytes between `VT` and `FS`.
//! * ASTM frame: `STX` ... (`ETX` | `ETB`) two hex checksum digits `CR` `LF`. The event carries
//!   the whole frame. A frame whose checksum does not match is reported as `AstmBadChecksum`.
//! * A standalone `ENQ` outside of any frame.
//! * A standalone `ACK` outside of any frame (used by the send queue, not by the rules).
//!
//! * A standalone `NAK` or `EOT` outside of any frame (the send queue stops on them).
//!
//! Line breaks and any other stray byte between frames are ignored. A new
//! `VT` / `STX` inside a frame restarts the frame (resync) and reports the interrupted one as
//! malformed. A frame bigger than [`MAX_FRAME_BYTES`] is dropped (everything up to the next
//! start byte is ignored). Nothing partial is ever reported as a frame.
//!
//! [`FrameBuffer::feed`] returns a [`FeedResult`]: the completed events plus two flags telling
//! whether the read only contains ASTM units (`astm_only`) and whether no partial frame remains
//! (`idle`), which decide how the read is reported (per ASTM unit, or as a raw chunk).

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

use crate::protocol::astm::astm_checksum_is_valid;
use crate::translate::ControlToken;

const STX: u8 = ControlToken::STX.byte();
const ETX: u8 = ControlToken::ETX.byte();
const ENQ: u8 = ControlToken::ENQ.byte();
const EOT: u8 = ControlToken::EOT.byte();
const ACK: u8 = ControlToken::ACK.byte();
const NAK: u8 = ControlToken::NAK.byte();
const VT: u8 = ControlToken::VT.byte();
const LF: u8 = ControlToken::LF.byte();
const CR: u8 = ControlToken::CR.byte();
const ETB: u8 = ControlToken::ETB.byte();
const FS: u8 = ControlToken::FS.byte();

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameEvent {
    /// Body of an MLLP frame (between `VT` and `FS`, segments separated by `CR`).
    Hl7(Vec<u8>),
    /// A whole ASTM frame, from `STX` to the final `LF`.
    AstmFrame(Vec<u8>),
    /// A standalone `ENQ`.
    Enq,
    /// A standalone `ACK` outside of any frame (an `ACK` inside a frame is payload).
    Ack,
    /// A standalone `NAK` outside of any frame.
    Nak,
    /// A standalone `EOT` outside of any frame.
    Eot,
    /// A well-formed ASTM frame whose checksum does not match. It is reported (so the UI can
    /// show it) and answered with a `NAK` by the orchestrator, never by a user rule.
    AstmBadChecksum(Vec<u8>),
    /// A frame that was abandoned: nothing is replied to it.
    Malformed(&'static str),
    /// A frame exceeding [`MAX_FRAME_BYTES`] was dropped.
    Overflow,
}

/// Result of [`FrameBuffer::feed`]: what one read completed, and how it can be reported.
pub struct FeedResult {
    /// The events completed by the fed bytes, in order (empty while a frame is still partial).
    pub events: Vec<FrameEvent>,
    /// Every fed byte belongs to an ASTM unit: a standalone ENQ / ACK / NAK / EOT, the bytes of
    /// an ASTM frame (including the ones continuing a frame begun by a previous read) or the
    /// CR / LF between units. It is `false` as soon as one byte is anything else (HL7, text,
    /// stray bytes) or when a frame is interrupted / dropped (malformed, overflow). Only an
    /// `astm_only` read may be reported unit by unit.
    pub astm_only: bool,
    /// The buffer holds no partial frame after the read: no HL7 / ASTM frame is being received
    /// (or discarded) and no byte is pending. When `false`, the end of the read is the
    /// beginning of a frame completed by a later read. The orchestrator uses it as follows:
    /// a received read is reported as ASTM units when it is `astm_only` and either completed
    /// an event or left a frame pending (the pending bytes are then reported with the
    /// frame, once complete; otherwise the read would not be reported at all); a sent item
    /// (fed to a fresh buffer, so never completed later) is reported as units only when it is
    /// also `idle`, so an unterminated frame stays a raw message.
    pub idle: bool,
}

impl FeedResult {
    /// Whether a *received* read must be reported unit by unit (ENQ, ACK, NAK, EOT, frames)
    /// rather than as a raw chunk: it holds only ASTM bytes and either completed at least one
    /// unit or left a frame pending (its bytes are reported with the frame once complete;
    /// otherwise the read would not be reported at all). An empty read is not reported as units.
    pub fn should_report_received_as_astm_units(&self) -> bool {
        self.astm_only && (!self.events.is_empty() || !self.idle)
    }

    /// Whether a *sent* item must be reported unit by unit. Sent items are fed to a fresh
    /// buffer and never completed later, so unlike a received read it must hold only complete
    /// ASTM units: at least one, and no pending partial frame (an unterminated frame stays a
    /// raw message).
    pub fn should_report_sent_as_astm_units(&self) -> bool {
        self.astm_only && self.idle && !self.events.is_empty()
    }
}

impl FrameEvent {
    /// Exact wire bytes of an ASTM unit event (frame, bad frame, ENQ, ACK, NAK, EOT).
    pub fn astm_bytes(&self) -> Option<Vec<u8>> {
        match self {
            FrameEvent::AstmFrame(frame) | FrameEvent::AstmBadChecksum(frame) => {
                Some(frame.clone())
            }
            FrameEvent::Enq => Some(vec![ENQ]),
            FrameEvent::Ack => Some(vec![ACK]),
            FrameEvent::Nak => Some(vec![NAK]),
            FrameEvent::Eot => Some(vec![EOT]),
            _ => None,
        }
    }
}

enum State {
    Idle,
    Hl7(Vec<u8>),
    Hl7End(Vec<u8>),
    Astm(Vec<u8>),
    /// After ETX/ETB: `usize` tail bytes (2 checksum digits, CR, LF) already accepted.
    AstmTail(Vec<u8>, usize),
    /// Too large frame: ignore everything until the next start byte.
    Discarding,
}

pub struct FrameBuffer {
    state: State,
    max: usize,
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self::with_limit(MAX_FRAME_BYTES)
    }

    pub fn with_limit(max: usize) -> Self {
        Self {
            state: State::Idle,
            max,
        }
    }

    /// Feeds raw bytes; returns, in order, the events they completed, and tells whether the
    /// bytes only belong to ASTM units (ENQ / ACK / NAK / EOT, frames, the ones still being
    /// received, and the CR / LF between them): only then can they be reported unit by unit
    /// instead of as one raw chunk. See [`FeedResult`].
    pub fn feed(&mut self, data: &[u8]) -> FeedResult {
        let mut events = Vec::new();
        let mut astm_only = true;
        for &byte in data {
            let accepted = match self.state {
                State::Idle => matches!(byte, ENQ | ACK | NAK | EOT | STX | CR | LF),
                State::Astm(_) | State::AstmTail(..) => true,
                State::Hl7(_) | State::Hl7End(_) | State::Discarding => false,
            };
            let before = events.len();
            self.push_byte(byte, &mut events);
            let broken = events[before..]
                .iter()
                .any(|e| matches!(e, FrameEvent::Malformed(_) | FrameEvent::Overflow));
            astm_only &= accepted && !broken;
        }
        FeedResult {
            events,
            astm_only,
            idle: matches!(self.state, State::Idle),
        }
    }

    fn idle(&mut self, byte: u8, out: &mut Vec<FrameEvent>) {
        match byte {
            ENQ => out.push(FrameEvent::Enq),
            ACK => out.push(FrameEvent::Ack),
            NAK => out.push(FrameEvent::Nak),
            EOT => out.push(FrameEvent::Eot),
            VT => self.state = State::Hl7(Vec::new()),
            STX => self.state = State::Astm(vec![STX]),
            _ => self.state = State::Idle,
        }
    }

    fn grow(&mut self, mut body: Vec<u8>, byte: u8, hl7: bool, out: &mut Vec<FrameEvent>) {
        if body.len() >= self.max {
            out.push(FrameEvent::Overflow);
            self.state = State::Discarding;
            return;
        }
        body.push(byte);
        self.state = if hl7 {
            State::Hl7(body)
        } else {
            State::Astm(body)
        };
    }

    fn push_byte(&mut self, byte: u8, out: &mut Vec<FrameEvent>) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => self.idle(byte, out),
            State::Discarding => {
                self.state = State::Discarding;
                match byte {
                    // A new ASTM communication starts: resync.
                    ENQ => {
                        out.push(FrameEvent::Enq);
                        self.state = State::Idle;
                    }
                    VT => self.state = State::Hl7(Vec::new()),
                    STX => self.state = State::Astm(vec![STX]),
                    _ => {}
                }
            }
            State::Hl7(body) => match byte {
                FS => self.state = State::Hl7End(body),
                VT => {
                    out.push(FrameEvent::Malformed(
                        "an HL7 frame was interrupted by a new one",
                    ));
                    self.state = State::Hl7(Vec::new());
                }
                STX => {
                    out.push(FrameEvent::Malformed(
                        "an HL7 frame was interrupted by an ASTM frame",
                    ));
                    self.state = State::Astm(vec![STX]);
                }
                _ => self.grow(body, byte, true, out),
            },
            State::Hl7End(body) => {
                if byte == CR {
                    out.push(FrameEvent::Hl7(body));
                } else {
                    out.push(FrameEvent::Malformed(
                        "an HL7 frame end is not followed by CR",
                    ));
                    self.idle(byte, out);
                }
            }
            State::Astm(mut body) => match byte {
                ETX | ETB => {
                    body.push(byte);
                    self.state = State::AstmTail(body, 0);
                }
                STX => {
                    out.push(FrameEvent::Malformed(
                        "an ASTM frame was interrupted by a new one",
                    ));
                    self.state = State::Astm(vec![STX]);
                }
                VT => {
                    out.push(FrameEvent::Malformed(
                        "an ASTM frame was interrupted by an HL7 frame",
                    ));
                    self.state = State::Hl7(Vec::new());
                }
                _ => self.grow(body, byte, false, out),
            },
            State::AstmTail(mut body, accepted) => {
                let valid = match accepted {
                    0 | 1 => byte.is_ascii_hexdigit(),
                    2 => byte == CR,
                    _ => byte == LF,
                };
                if !valid {
                    out.push(FrameEvent::Malformed(
                        "an ASTM frame has an invalid checksum or terminator",
                    ));
                    self.idle(byte, out);
                } else {
                    body.push(byte);
                    if accepted == 3 {
                        if astm_checksum_is_valid(&body) {
                            out.push(FrameEvent::AstmFrame(body));
                        } else {
                            out.push(FrameEvent::AstmBadChecksum(body));
                        }
                    } else {
                        self.state = State::AstmTail(body, accepted + 1);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::astm::astm_checksum;

    const HL7: &[u8] = b"\x0bMSH|^~\\&|A|B|C|D|1||ADT^A01|X1|P|2.5\rPID|1\r\x1c\r";

    #[test]
    fn hl7_frame_split_everywhere_is_reported_once_when_complete() {
        for cut in 1..HL7.len() {
            let mut buffer = FrameBuffer::new();
            assert!(buffer.feed(&HL7[..cut]).events.is_empty(), "cut {cut}");
            let events = buffer.feed(&HL7[cut..]).events;
            assert_eq!(events.len(), 1, "cut {cut}");
            assert!(matches!(&events[0], FrameEvent::Hl7(body) if body.starts_with(b"MSH|")));
        }
    }

    #[test]
    fn coalesced_frames_are_reported_separately_and_in_order() {
        let mut data = HL7.to_vec();
        data.extend_from_slice(&[ENQ]);
        data.extend_from_slice(HL7);
        let events = FrameBuffer::new().feed(&data).events;
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], FrameEvent::Hl7(_)));
        assert_eq!(events[1], FrameEvent::Enq);
        assert!(matches!(events[2], FrameEvent::Hl7(_)));
    }

    /// Builds a valid frame `STX body ETX checksum CR LF` (`body` includes its trailing CR).
    fn astm(body: &[u8]) -> Vec<u8> {
        let mut content = body.to_vec();
        content.push(ETX);
        let mut frame = vec![STX];
        frame.extend_from_slice(&content);
        frame.extend_from_slice(format!("{:02X}\r\n", astm_checksum(&content)).as_bytes());
        frame
    }

    #[test]
    fn wrong_checksum_is_reported_as_a_bad_frame() {
        let events = FrameBuffer::new().feed(b"\x021H|x\r\x0300\r\n").events;
        assert_eq!(
            events,
            vec![FrameEvent::AstmBadChecksum(
                b"\x021H|x\r\x0300\r\n".to_vec()
            )]
        );
    }

    #[test]
    fn feed_keeps_only_astm_reads_unit_by_unit() {
        let frame = astm(b"1H|\\^&|||x\r");
        let mut data = vec![ENQ];
        data.extend_from_slice(&frame);
        data.extend_from_slice(b"\r\n");
        let feed_result = FrameBuffer::new().feed(&data);
        assert!(feed_result.astm_only && feed_result.idle);
        assert_eq!(
            feed_result.events,
            vec![FrameEvent::Enq, FrameEvent::AstmFrame(frame)]
        );

        assert!(!FrameBuffer::new().feed(HL7).astm_only);
        assert!(!FrameBuffer::new().feed(b"hello").astm_only);
        assert!(FrameBuffer::new().feed(b"\x021H|x\r\x0300\r\n").astm_only);
    }

    #[test]
    fn feed_reports_a_frame_split_over_reads() {
        let frame = astm(b"1H|\\^&|||x\r");
        let mut buffer = FrameBuffer::new();
        let first = buffer.feed(&frame[..5]);
        assert!(first.astm_only && !first.idle && first.events.is_empty());
        let second = buffer.feed(&frame[5..]);
        assert!(second.astm_only && second.idle);
        assert_eq!(second.events, vec![FrameEvent::AstmFrame(frame)]);
    }

    #[test]
    fn feed_rejects_interrupted_frames() {
        let feed_result = FrameBuffer::new().feed(b"\x021H|\x021H|\r");
        assert!(!feed_result.astm_only);
    }

    #[test]
    fn astm_units_reporting_for_complete_astm_units() {
        let mut data = vec![ENQ];
        data.extend_from_slice(&astm(b"1H|\\^&|||x\r"));
        let feed_result = FrameBuffer::new().feed(&data);
        assert!(feed_result.should_report_received_as_astm_units());
        assert!(feed_result.should_report_sent_as_astm_units());
    }

    #[test]
    fn astm_units_reporting_with_pending_partial_frame() {
        let frame = astm(b"1H|\\^&|||x\r");
        // Received: the pending bytes are reported with the frame once complete.
        let partial = FrameBuffer::new().feed(&frame[..5]);
        assert!(partial.should_report_received_as_astm_units());
        // Sent: an unterminated frame stays a raw message.
        assert!(!partial.should_report_sent_as_astm_units());
        // Units followed by a partial frame: the sent item is not purely complete units.
        let mut data = vec![ENQ];
        data.extend_from_slice(&frame[..5]);
        let mixed_pending = FrameBuffer::new().feed(&data);
        assert!(mixed_pending.should_report_received_as_astm_units());
        assert!(!mixed_pending.should_report_sent_as_astm_units());
    }

    #[test]
    fn astm_units_reporting_with_foreign_bytes() {
        let mut data = vec![ENQ];
        data.extend_from_slice(b"hello");
        let feed_result = FrameBuffer::new().feed(&data);
        assert!(!feed_result.should_report_received_as_astm_units());
        assert!(!feed_result.should_report_sent_as_astm_units());
        let hl7 = FrameBuffer::new().feed(HL7);
        assert!(!hl7.should_report_received_as_astm_units());
        assert!(!hl7.should_report_sent_as_astm_units());
    }

    #[test]
    fn astm_units_reporting_for_empty_read() {
        let feed_result = FrameBuffer::new().feed(b"");
        assert!(!feed_result.should_report_received_as_astm_units());
        assert!(!feed_result.should_report_sent_as_astm_units());
    }

    #[test]
    fn enq_ends_discarding_state() {
        let mut buffer = FrameBuffer::with_limit(8);
        let mut data = vec![VT];
        data.extend_from_slice(&[b'a'; 20]);
        data.push(ENQ);
        let events = buffer.feed(&data).events;
        assert_eq!(events, vec![FrameEvent::Overflow, FrameEvent::Enq]);
        assert_eq!(buffer.feed(&[ENQ]).events, vec![FrameEvent::Enq]);
    }

    #[test]
    fn astm_frame_is_reported_only_once_checksum_and_terminator_arrived() {
        let frame = &astm(b"1H|\\^&|||x\r")[..];
        let mut buffer = FrameBuffer::new();
        assert!(buffer.feed(&frame[..frame.len() - 3]).events.is_empty());
        assert!(buffer
            .feed(&frame[frame.len() - 3..frame.len() - 1])
            .events
            .is_empty());
        let events = buffer.feed(&frame[frame.len() - 1..]).events;
        assert_eq!(events, vec![FrameEvent::AstmFrame(frame.to_vec())]);
    }

    #[test]
    fn control_bytes_outside_frames_are_never_triggers() {
        let feed_result = FrameBuffer::new().feed(b"\r\nx");
        assert!(feed_result.events.is_empty());
        assert_eq!(
            FrameBuffer::new().feed(&[ENQ]).events,
            vec![FrameEvent::Enq]
        );
    }

    #[test]
    fn standalone_acks_surface_in_order_even_when_coalesced() {
        assert_eq!(
            FrameBuffer::new().feed(&[ACK]).events,
            vec![FrameEvent::Ack]
        );
        assert_eq!(
            FrameBuffer::new().feed(&[ACK, ACK, ENQ, ACK]).events,
            vec![
                FrameEvent::Ack,
                FrameEvent::Ack,
                FrameEvent::Enq,
                FrameEvent::Ack
            ]
        );
        let mut data = vec![ACK];
        data.extend_from_slice(HL7);
        data.push(ACK);
        let events = FrameBuffer::new().feed(&data).events;
        assert!(matches!(
            events[..],
            [FrameEvent::Ack, FrameEvent::Hl7(_), FrameEvent::Ack]
        ));
        let mut astm_data = vec![ACK];
        astm_data.extend(astm(b"1H|x\r"));
        let events = FrameBuffer::new().feed(&astm_data).events;
        assert!(matches!(
            events[..],
            [FrameEvent::Ack, FrameEvent::AstmFrame(_)]
        ));
    }

    #[test]
    fn standalone_nak_and_eot_surface_but_are_payload_inside_frames() {
        assert_eq!(
            FrameBuffer::new().feed(&[NAK, EOT, b'x', NAK]).events,
            vec![FrameEvent::Nak, FrameEvent::Eot, FrameEvent::Nak]
        );
        let events = FrameBuffer::new().feed(b"\x0bMSH|a\x15b\x04\x1c\r").events;
        assert!(matches!(events[..], [FrameEvent::Hl7(_)]));
        let events = FrameBuffer::new().feed(&astm(b"1H|\x15\x04\r")).events;
        assert!(matches!(events[..], [FrameEvent::AstmFrame(_)]));
    }

    #[test]
    fn ack_bytes_inside_a_frame_are_payload() {
        let events = FrameBuffer::new().feed(b"\x0bMSH|a\x06b\x1c\r").events;
        assert!(matches!(events[..], [FrameEvent::Hl7(_)]));
        let events = FrameBuffer::new().feed(&astm(b"1H|\x06\r")).events;
        assert!(matches!(events[..], [FrameEvent::AstmFrame(_)]));
    }

    #[test]
    fn a_new_start_byte_resyncs_and_reports_the_broken_frame() {
        let mut data = b"\x0bMSH|broken".to_vec();
        data.extend_from_slice(HL7);
        let events = FrameBuffer::new().feed(&data).events;
        assert!(matches!(events[0], FrameEvent::Malformed(_)));
        assert!(matches!(events[1], FrameEvent::Hl7(_)));
        assert_eq!(events.len(), 2);

        let events = FrameBuffer::new().feed(b"\x021H|x\r\x03ZZ\r\n").events;
        assert!(matches!(events[0], FrameEvent::Malformed(_)));
        assert_eq!(events.len(), 1);

        let events = FrameBuffer::new().feed(b"\x0bMSH|x\x1cX").events;
        assert!(matches!(events[0], FrameEvent::Malformed(_)));
    }

    #[test]
    fn oversized_frames_are_dropped_until_the_next_start_byte() {
        let mut buffer = FrameBuffer::with_limit(8);
        let mut data = vec![VT];
        data.extend_from_slice(&[b'a'; 20]);
        data.push(ENQ);
        let events = buffer.feed(&data).events;
        assert_eq!(events, vec![FrameEvent::Overflow, FrameEvent::Enq]);
        let events = buffer.feed(HL7).events;
        // The HL7 frame itself is also larger than 8 bytes: it overflows too, nothing partial.
        assert_eq!(events, vec![FrameEvent::Overflow]);
        let mut buffer = FrameBuffer::with_limit(256);
        assert!(matches!(buffer.feed(HL7).events[0], FrameEvent::Hl7(_)));
    }
}
