//! Incremental extraction of complete protocol messages from a raw TCP byte stream.
//!
//! The UI keeps showing the raw chunks as they arrive; automatic replies must only react to
//! COMPLETE messages, whatever the way TCP split or coalesced them:
//!
//! * HL7 / MLLP: `VT` ... `FS` `CR`. The event carries the bytes between `VT` and `FS`.
//! * ASTM frame: `STX` ... (`ETX` | `ETB`) two hex checksum digits `CR` `LF`. The event carries
//!   the whole frame. The checksum value itself is not verified (this is a simulator that must
//!   cope with sloppy peers), only the structure is.
//! * A standalone `ENQ` outside of any frame.
//! * A standalone `ACK` outside of any frame (used by the send queue, not by the rules).
//!
//! `NAK`, `EOT`, line breaks and any other stray byte between frames are ignored. A new
//! `VT` / `STX` inside a frame restarts the frame (resync) and reports the interrupted one as
//! malformed. A frame bigger than [`MAX_FRAME_BYTES`] is dropped (everything up to the next
//! start byte is ignored). Nothing partial is ever reported as a frame.

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const ENQ: u8 = 0x05;
const ACK: u8 = 0x06;
const VT: u8 = 0x0b;
const LF: u8 = 0x0a;
const CR: u8 = 0x0d;
const ETB: u8 = 0x17;
const FS: u8 = 0x1c;

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
    /// A frame that was abandoned: nothing is replied to it.
    Malformed(&'static str),
    /// A frame exceeding [`MAX_FRAME_BYTES`] was dropped.
    Overflow,
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

    /// Feeds raw bytes; returns, in order, the events they completed.
    pub fn feed(&mut self, data: &[u8]) -> Vec<FrameEvent> {
        let mut out = Vec::new();
        for &byte in data {
            self.push_byte(byte, &mut out);
        }
        out
    }

    fn idle(&mut self, byte: u8, out: &mut Vec<FrameEvent>) {
        match byte {
            ENQ => out.push(FrameEvent::Enq),
            ACK => out.push(FrameEvent::Ack),
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
                        out.push(FrameEvent::AstmFrame(body));
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

    const HL7: &[u8] = b"\x0bMSH|^~\\&|A|B|C|D|1||ADT^A01|X1|P|2.5\rPID|1\r\x1c\r";

    #[test]
    fn hl7_frame_split_everywhere_is_reported_once_when_complete() {
        for cut in 1..HL7.len() {
            let mut buffer = FrameBuffer::new();
            assert!(buffer.feed(&HL7[..cut]).is_empty(), "cut {cut}");
            let events = buffer.feed(&HL7[cut..]);
            assert_eq!(events.len(), 1, "cut {cut}");
            assert!(matches!(&events[0], FrameEvent::Hl7(body) if body.starts_with(b"MSH|")));
        }
    }

    #[test]
    fn coalesced_frames_are_reported_separately_and_in_order() {
        let mut data = HL7.to_vec();
        data.extend_from_slice(&[ENQ]);
        data.extend_from_slice(HL7);
        let events = FrameBuffer::new().feed(&data);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], FrameEvent::Hl7(_)));
        assert_eq!(events[1], FrameEvent::Enq);
        assert!(matches!(events[2], FrameEvent::Hl7(_)));
    }

    #[test]
    fn astm_frame_is_reported_only_once_checksum_and_terminator_arrived() {
        let frame = b"\x021H|\\^&|||x\r\x0342\r\n";
        let mut buffer = FrameBuffer::new();
        assert!(buffer.feed(&frame[..frame.len() - 3]).is_empty());
        assert!(buffer
            .feed(&frame[frame.len() - 3..frame.len() - 1])
            .is_empty());
        let events = buffer.feed(&frame[frame.len() - 1..]);
        assert_eq!(events, vec![FrameEvent::AstmFrame(frame.to_vec())]);
    }

    #[test]
    fn control_bytes_outside_frames_are_never_triggers() {
        let events = FrameBuffer::new().feed(&[0x15, 0x04, b'\r', b'\n', b'x']);
        assert!(events.is_empty());
        assert_eq!(FrameBuffer::new().feed(&[ENQ]), vec![FrameEvent::Enq]);
    }

    #[test]
    fn standalone_acks_surface_in_order_even_when_coalesced() {
        assert_eq!(FrameBuffer::new().feed(&[ACK]), vec![FrameEvent::Ack]);
        assert_eq!(
            FrameBuffer::new().feed(&[ACK, ACK, ENQ, ACK]),
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
        let events = FrameBuffer::new().feed(&data);
        assert!(matches!(
            events[..],
            [FrameEvent::Ack, FrameEvent::Hl7(_), FrameEvent::Ack]
        ));
        let astm = b"\x06\x021H|x\r\x0342\r\n";
        let events = FrameBuffer::new().feed(astm);
        assert!(matches!(
            events[..],
            [FrameEvent::Ack, FrameEvent::AstmFrame(_)]
        ));
    }

    #[test]
    fn ack_bytes_inside_a_frame_are_payload() {
        let events = FrameBuffer::new().feed(b"\x0bMSH|a\x06b\x1c\r");
        assert!(matches!(events[..], [FrameEvent::Hl7(_)]));
        let events = FrameBuffer::new().feed(b"\x021H|\x06\r\x0342\r\n");
        assert!(matches!(events[..], [FrameEvent::AstmFrame(_)]));
    }

    #[test]
    fn a_new_start_byte_resyncs_and_reports_the_broken_frame() {
        let mut data = b"\x0bMSH|broken".to_vec();
        data.extend_from_slice(HL7);
        let events = FrameBuffer::new().feed(&data);
        assert!(matches!(events[0], FrameEvent::Malformed(_)));
        assert!(matches!(events[1], FrameEvent::Hl7(_)));
        assert_eq!(events.len(), 2);

        let events = FrameBuffer::new().feed(b"\x021H|x\r\x03ZZ\r\n");
        assert!(matches!(events[0], FrameEvent::Malformed(_)));
        assert_eq!(events.len(), 1);

        let events = FrameBuffer::new().feed(b"\x0bMSH|x\x1cX");
        assert!(matches!(events[0], FrameEvent::Malformed(_)));
    }

    #[test]
    fn oversized_frames_are_dropped_until_the_next_start_byte() {
        let mut buffer = FrameBuffer::with_limit(8);
        let mut data = vec![VT];
        data.extend_from_slice(&[b'a'; 20]);
        data.push(ENQ);
        let events = buffer.feed(&data);
        assert_eq!(events, vec![FrameEvent::Overflow]);
        let events = buffer.feed(HL7);
        // The HL7 frame itself is also larger than 8 bytes: it overflows too, nothing partial.
        assert_eq!(events, vec![FrameEvent::Overflow]);
        let mut buffer = FrameBuffer::with_limit(256);
        assert!(matches!(buffer.feed(HL7)[0], FrameEvent::Hl7(_)));
    }
}
