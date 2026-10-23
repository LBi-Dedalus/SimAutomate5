//! Protocol-level classification of outgoing frames.

use crate::translate::ControlToken;

/// Whether the frame starts an ASTM exchange (ENQ / STX) whose ACK must be awaited.
pub(super) fn requires_ack(line: &[u8]) -> bool {
    if line.is_empty() {
        return false;
    }

    line[0] == ControlToken::ENQ.byte() || line[0] == ControlToken::STX.byte()
}

/// A pure acknowledgement (`ACK` / `NAK`): the only automatic reply allowed to bypass the wait
/// for the ACK of our own transmission.
pub(super) fn is_control_reply(frame: &[u8]) -> bool {
    frame == [ControlToken::ACK.byte()] || frame == [ControlToken::NAK.byte()]
}
