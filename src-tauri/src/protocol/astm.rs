//! ASTM checksum computation and verification, and the description of ASTM units for the UI.

use serde::Serialize;

use crate::protocol::frames::FrameEvent;
use crate::translate::ControlToken;

/// One unit of an ASTM exchange, as seen on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AstmUnit {
    Enq,
    Ack,
    Nak,
    Eot,
    Frame(AstmFrameInfo),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AstmFrameInfo {
    /// Frame number digit following `STX`, when present.
    pub number: Option<u8>,
    /// Text of the frame: without `STX`, the frame number, `ETX`/`ETB`, checksum and `CR LF`.
    /// Its records are separated by `CR`.
    pub text: String,
    /// `ETX` (last frame of the message) or `ETB` (intermediate frame).
    pub terminator: &'static str,
    /// Checksum digits as received.
    pub checksum: String,
    /// Checksum computed from the frame content.
    pub computed_checksum: String,
    pub checksum_ok: bool,
}

/// ASTM checksum: sum modulo 256 of the bytes following `STX`, up to and including `ETX` / `ETB`.
pub(crate) fn astm_checksum(content: &[u8]) -> u8 {
    content.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
}

/// Describes a whole frame `STX [number] text (ETX|ETB) C1 C2 CR LF` as extracted by
/// `FrameBuffer` (so at least `STX`, the end byte, 2 digits, `CR` and `LF`).
fn describe_frame(frame: &[u8]) -> AstmFrameInfo {
    let end = frame.len() - 5;
    let number = frame
        .get(1)
        .filter(|b| end > 1 && b.is_ascii_digit())
        .map(|b| b - b'0');
    let text_start = if number.is_some() { 2 } else { 1 };
    let text = String::from_utf8_lossy(&frame[text_start.min(end)..end]).into_owned();
    let terminator = if frame[end] == ControlToken::ETB.byte() {
        "ETB"
    } else {
        "ETX"
    };
    let checksum = String::from_utf8_lossy(&frame[end + 1..end + 3]).to_ascii_uppercase();
    let computed = format!("{:02X}", astm_checksum(&frame[1..=end]));
    AstmFrameInfo {
        number,
        text,
        terminator,
        checksum_ok: checksum == computed,
        checksum,
        computed_checksum: computed,
    }
}

/// The ASTM unit an event stands for, with its exact wire bytes.
pub(crate) fn astm_unit(event: &FrameEvent) -> Option<(AstmUnit, Vec<u8>)> {
    let bytes = event.astm_bytes()?;
    let unit = match event {
        FrameEvent::Enq => AstmUnit::Enq,
        FrameEvent::Ack => AstmUnit::Ack,
        FrameEvent::Nak => AstmUnit::Nak,
        FrameEvent::Eot => AstmUnit::Eot,
        _ => AstmUnit::Frame(describe_frame(&bytes)),
    };
    Some((unit, bytes))
}

/// Verifies a whole frame `STX ... (ETX|ETB) C1 C2 CR LF` against its two hex checksum digits.
pub(crate) fn astm_checksum_is_valid(frame: &[u8]) -> bool {
    // STX + end byte + 2 digits + CR + LF
    if frame.len() < 6 {
        return false;
    }
    let digits_at = frame.len() - 4;
    let expected = std::str::from_utf8(&frame[digits_at..digits_at + 2])
        .ok()
        .and_then(|s| u8::from_str_radix(s, 16).ok());
    expected == Some(astm_checksum(&frame[1..digits_at]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(content: &[u8], digits: &str) -> Vec<u8> {
        let mut frame = vec![0x02];
        frame.extend_from_slice(content);
        frame.extend_from_slice(digits.as_bytes());
        frame.extend_from_slice(b"\r\n");
        frame
    }

    #[test]
    fn describes_frame_number_text_terminator_and_checksum() {
        let content = b"3P|1\r\x17";
        let digits = format!("{:02x}", astm_checksum(content));
        let (unit, bytes) = astm_unit(&FrameEvent::AstmFrame(frame(content, &digits))).unwrap();
        assert_eq!(bytes, frame(content, &digits));
        let AstmUnit::Frame(info) = unit else {
            panic!("a frame is expected");
        };
        assert_eq!(info.number, Some(3));
        assert_eq!(info.text, "P|1\r");
        assert_eq!(info.terminator, "ETB");
        assert!(info.checksum_ok);
        assert_eq!(info.checksum, digits.to_uppercase());
    }

    #[test]
    fn degenerate_frames_do_not_panic() {
        for bytes in [
            &b"\x02\x0300\r\n"[..],
            b"\x021\x0300\r\n",
            b"\x02x\x0300\r\n",
        ] {
            let Some((AstmUnit::Frame(info), _)) =
                astm_unit(&FrameEvent::AstmBadChecksum(bytes.to_vec()))
            else {
                panic!("a frame is expected");
            };
            assert_eq!(info.text, if bytes[1] == b'x' { "x" } else { "" });
        }
    }

    #[test]
    fn control_events_are_units_with_their_byte() {
        assert_eq!(
            astm_unit(&FrameEvent::Enq),
            Some((AstmUnit::Enq, vec![0x05]))
        );
        assert_eq!(
            astm_unit(&FrameEvent::Ack),
            Some((AstmUnit::Ack, vec![0x06]))
        );
        assert_eq!(
            astm_unit(&FrameEvent::Nak),
            Some((AstmUnit::Nak, vec![0x15]))
        );
        assert_eq!(
            astm_unit(&FrameEvent::Eot),
            Some((AstmUnit::Eot, vec![0x04]))
        );
        assert_eq!(astm_unit(&FrameEvent::Hl7(vec![])), None);
    }
}
