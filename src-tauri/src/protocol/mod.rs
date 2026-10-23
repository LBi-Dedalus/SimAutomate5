//! Wire protocol helpers: classification of frames, ASTM checksums and HL7 parsing.

pub(crate) mod astm;
pub(crate) mod hl7;

use crate::translate::ControlToken;

/// Whether the frame starts an ASTM exchange (ENQ / STX) whose ACK must be awaited.
pub(crate) fn requires_ack(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }

    line[0] == ControlToken::ENQ.byte() || line[0] == ControlToken::STX.byte()
}

/// A pure acknowledgement (`ACK` / `NAK`): the only automatic reply allowed to bypass the wait
/// for the ACK of our own transmission.
pub(crate) fn is_control_reply(frame: &[u8]) -> bool {
    frame == [ControlToken::ACK.byte()] || frame == [ControlToken::NAK.byte()]
}
