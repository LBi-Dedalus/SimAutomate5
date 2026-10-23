#[derive(Debug, Clone, Copy)]
pub enum ControlToken {
    NUL = 0x00,
    SOH = 0x01,
    STX = 0x02,
    ETX = 0x03,
    EOT = 0x04,
    ENQ = 0x05,
    ACK = 0x06,
    BEL = 0x07,
    BS = 0x08,
    HT = 0x09,
    LF = 0x0A,
    VT = 0x0B,
    FF = 0x0C,
    CR = 0x0D,
    SO = 0x0E,
    SI = 0x0F,
    DLE = 0x10,
    DC1 = 0x11,
    DC2 = 0x12,
    DC3 = 0x13,
    DC4 = 0x14,
    NAK = 0x15,
    SYN = 0x16,
    ETB = 0x17,
    CAN = 0x18,
    EM = 0x19,
    SUB = 0x1A,
    ESC = 0x1B,
    FS = 0x1C,
    GS = 0x1D,
    RS = 0x1E,
    US = 0x1F,
}

const CONTROL_TOKENS: [(&str, ControlToken); 32] = [
    ("<NUL>", ControlToken::NUL),
    ("<SOH>", ControlToken::SOH),
    ("<STX>", ControlToken::STX),
    ("<ETX>", ControlToken::ETX),
    ("<EOT>", ControlToken::EOT),
    ("<ENQ>", ControlToken::ENQ),
    ("<ACK>", ControlToken::ACK),
    ("<BEL>", ControlToken::BEL),
    ("<BS>", ControlToken::BS),
    ("<HT>", ControlToken::HT),
    ("<LF>", ControlToken::LF),
    ("<VT>", ControlToken::VT),
    ("<FF>", ControlToken::FF),
    ("<CR>", ControlToken::CR),
    ("<SO>", ControlToken::SO),
    ("<SI>", ControlToken::SI),
    ("<DLE>", ControlToken::DLE),
    ("<DC1>", ControlToken::DC1),
    ("<DC2>", ControlToken::DC2),
    ("<DC3>", ControlToken::DC3),
    ("<DC4>", ControlToken::DC4),
    ("<NAK>", ControlToken::NAK),
    ("<SYN>", ControlToken::SYN),
    ("<ETB>", ControlToken::ETB),
    ("<CAN>", ControlToken::CAN),
    ("<EM>", ControlToken::EM),
    ("<SUB>", ControlToken::SUB),
    ("<ESC>", ControlToken::ESC),
    ("<FS>", ControlToken::FS),
    ("<GS>", ControlToken::GS),
    ("<RS>", ControlToken::RS),
    ("<US>", ControlToken::US),
];

/// Byte length of the longest entry in `CONTROL_TOKENS` (e.g. "<NUL>").
const MAX_TOKEN_LEN: usize = {
    let mut max = 0;
    let mut i = 0;
    while i < CONTROL_TOKENS.len() {
        let len = CONTROL_TOKENS[i].0.len();
        if len > max {
            max = len;
        }
        i += 1;
    }
    max
};

impl ControlToken {
    pub const fn byte(self) -> u8 {
        self as u8
    }

    pub const fn char(self) -> char {
        self as u8 as char
    }
}

/// Encoder from text containing `<TOKEN>` control tokens to wire bytes (UTF-8).
pub struct ControlTokenEncoder;

impl ControlTokenEncoder {
    pub fn encode_all(text: &str) -> Vec<u8> {
        encode_prefix(text, true).0
    }
}

/// Encodes `input`; returns the bytes and the number of trailing bytes of `input` held
/// back (with `final_input == false`, a trailing `<...` shorter than any full token
/// and without `>` may still become a token).
fn encode_prefix(input: &str, final_input: bool) -> (Vec<u8>, usize) {
    // Text is UTF-8 on the wire; `<` and `>` are ASCII so slicing is safe.
    let mut out = Vec::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.extend_from_slice(&rest.as_bytes()[..start]);
        let tail = &rest[start..];
        // A token is at most MAX_TOKEN_LEN bytes, so only look that far for `>`.
        let window = &tail.as_bytes()[..tail.len().min(MAX_TOKEN_LEN)];
        match window.iter().position(|&b| b == b'>') {
            Some(end) => {
                // `>` is ASCII, so `end + 1` is a char boundary.
                if let Some(b) = token_to_byte(&tail[..=end]) {
                    out.push(b);
                    rest = &tail[end + 1..];
                    continue;
                }
            }
            None if !final_input && tail.len() < MAX_TOKEN_LEN => {
                return (out, tail.len());
            }
            None => {}
        }
        out.push(b'<');
        rest = &tail[1..];
    }
    out.extend_from_slice(rest.as_bytes());
    (out, 0)
}

/// Decodes `bytes`; returns the text and the number of trailing bytes held back.
/// With `final_input == false`, a trailing incomplete-but-possibly-valid sequence is not
/// decoded (it is the only thing ever held back, at most 3 bytes).
fn decode_prefix(bytes: &[u8], final_input: bool) -> (String, usize) {
    let mut result = String::with_capacity(bytes.len());
    let mut chunks = bytes.utf8_chunks().peekable();
    let mut held = 0;
    while let Some(chunk) = chunks.next() {
        result.push_str(&escape_controls(chunk.valid()));
        let invalid = chunk.invalid();
        if invalid.is_empty() {
            continue;
        }
        // An incomplete sequence can only be reported by the last chunk.
        if !final_input && chunks.peek().is_none() && is_incomplete_prefix(invalid) {
            held = invalid.len();
        } else {
            result.push('\u{FFFD}');
        }
    }
    (result, held)
}

/// True if `invalid` (the invalid part of the last chunk) is a valid UTF-8 prefix that
/// merely lacks continuation bytes. `utf8_chunks` reports such a tail as a single chunk
/// whose invalid part is shorter than the sequence length implied by its lead byte.
fn is_incomplete_prefix(invalid: &[u8]) -> bool {
    let needed = match invalid.first() {
        Some(0xC2..=0xDF) => 2,
        Some(0xE0..=0xEF) => 3,
        Some(0xF0..=0xF4) => 4,
        _ => return false,
    };
    // A lone-error chunk (e.g. `C3 41` -> invalid `C3`) is always followed by more bytes,
    // so reaching the end of input with a short invalid part means truncation.
    invalid.len() < needed
}

/// Incremental UTF-8 decoder for a byte stream read in arbitrary chunks. A multibyte
/// character split across chunks is reassembled; genuinely invalid bytes become U+FFFD
/// immediately. Control bytes are shown as `<TOKEN>`; invalid sequences as U+FFFD
/// (the Hex inspector shows exact bytes).
#[derive(Debug, Default)]
pub struct ControlTokenDecoder {
    pending: Vec<u8>,
}

impl ControlTokenDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// One-shot decoding of a complete byte buffer.
    pub fn decode_all(bytes: &[u8]) -> String {
        decode_prefix(bytes, true).0
    }

    pub fn decode(&mut self, chunk: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        let (text, held) = decode_prefix(&data, false);
        self.pending = data[data.len() - held..].to_vec();
        text
    }

    /// Number of bytes currently held back awaiting continuation.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Flushes at end of stream: a dangling partial sequence becomes U+FFFD.
    pub fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            "\u{FFFD}".to_string()
        }
    }
}

/// Replaces control characters 0x00-0x1F with their `<TOKEN>` form.
fn escape_controls(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        match u8::try_from(ch).ok().and_then(byte_to_token) {
            Some(token) => result.push_str(token),
            None => result.push(ch),
        }
    }
    result
}

fn token_to_byte(token: &str) -> Option<u8> {
    CONTROL_TOKENS
        .iter()
        .find(|(t, _)| *t == token)
        .map(|(_, b)| b.byte())
}

fn byte_to_token(byte: u8) -> Option<&'static str> {
    CONTROL_TOKENS
        .iter()
        .find(|(_, b)| b.byte() == byte)
        .map(|(t, _)| *t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_bytes(s: &str) -> Vec<u8> {
        ControlTokenEncoder::encode_all(s)
    }

    fn to_human_readable(b: &[u8]) -> String {
        ControlTokenDecoder::decode_all(b)
    }

    #[test]
    fn ascii_and_tokens() {
        assert_eq!(to_bytes("A<STX>B<x>"), b"A\x02B<x>");
        assert_eq!(to_bytes("<<STX>"), b"<\x02");
        assert_eq!(to_bytes("a<b"), b"a<b");
        assert_eq!(to_bytes("<"), b"<");
    }

    #[test]
    fn malformed_inputs() {
        assert_eq!(to_bytes("<<<<"), b"<<<<");
        assert_eq!(to_bytes("<<<STX>"), b"<<\x02");
        assert_eq!(to_bytes("<STX"), b"<STX");
        assert_eq!(to_bytes("<ABCDEF>"), b"<ABCDEF>");
        assert_eq!(to_bytes("<NUL"), b"<NUL");
        assert_eq!(to_bytes("<NUL>>"), b"\x00>");
    }

    #[test]
    fn multibyte_near_brackets() {
        assert_eq!(to_bytes("<é>"), "<é>".as_bytes());
        assert_eq!(to_bytes("<😀"), "<😀".as_bytes());
        assert_eq!(to_bytes("<é€"), "<é€".as_bytes());
        assert_eq!(to_bytes("<😀>"), "<😀>".as_bytes());
        assert_eq!(to_bytes("<é<STX>"), b"<\xC3\xA9\x02");
        assert_eq!(to_bytes("<€€€"), "<€€€".as_bytes());
    }

    #[test]
    fn max_token_len_matches_table() {
        assert!(CONTROL_TOKENS.iter().all(|(t, _)| t.len() <= MAX_TOKEN_LEN));
        assert!(CONTROL_TOKENS.iter().any(|(t, _)| t.len() == MAX_TOKEN_LEN));
    }

    #[test]
    fn large_input_of_open_brackets() {
        let input = "<".repeat(1_000_000);
        let out = to_bytes(&input);
        assert_eq!(out.len(), 1_000_000);
        assert!(out.iter().all(|&b| b == b'<'));
    }

    #[test]
    fn unicode_is_utf8_encoded() {
        assert_eq!(to_bytes("é"), [0xC3, 0xA9]);
        let a = to_bytes("ą");
        assert_eq!(a, [0xC4, 0x85]);
        assert!(!a.contains(&0x05));
        assert_eq!(to_bytes("€"), [0xE2, 0x82, 0xAC]);
        assert_eq!(to_bytes("😀"), [0xF0, 0x9F, 0x98, 0x80]);
        assert_eq!(to_bytes("é<ETX>€"), [0xC3, 0xA9, 0x03, 0xE2, 0x82, 0xAC]);
    }

    #[test]
    fn decoding() {
        assert_eq!(to_human_readable(&[0x02, 0xC3, 0xA9]), "<STX>é");
        assert_eq!(to_human_readable(&[0x41, 0xFF]), "A\u{FFFD}");
        assert_eq!(to_human_readable(b"a\nb"), "a<LF>b");
    }

    fn split_decode(data: &[u8], at: usize) -> String {
        let mut d = ControlTokenDecoder::new();
        let mut out = d.decode(&data[..at]);
        out.push_str(&d.decode(&data[at..]));
        out.push_str(&d.finish());
        out
    }

    #[test]
    fn stream_split_at_every_boundary() {
        for ch in ["é", "€", "😀"] {
            let mut data = vec![0x02];
            data.extend(to_bytes(&format!("A{ch}B")));
            let expected = to_human_readable(&data);
            assert_eq!(expected, format!("<STX>A{ch}B"));
            for at in 0..=data.len() {
                assert_eq!(split_decode(&data, at), expected, "char {ch} split {at}");
            }
        }
    }

    #[test]
    fn stream_byte_by_byte() {
        let data = to_bytes("<STX>Café €😀\u{FF}<ETX>");
        let mut d = ControlTokenDecoder::new();
        let mut out = String::new();
        for b in &data {
            out.push_str(&d.decode(&[*b]));
        }
        out.push_str(&d.finish());
        assert_eq!(out, to_human_readable(&data));
    }

    #[test]
    fn stream_invalid_bytes() {
        let mut d = ControlTokenDecoder::new();
        assert_eq!(d.decode(&[0x41, 0xFF, 0x42]), "A\u{FFFD}B");
        assert_eq!(d.pending_len(), 0);
        assert_eq!(d.decode(&[0xC3]), "");
        assert_eq!(d.pending_len(), 1);
        assert_eq!(d.finish(), "\u{FFFD}");
        assert_eq!(d.finish(), "");
        assert_eq!(d.decode(&[0xC3, 0x41]), "\u{FFFD}A");
        // Held bytes followed by an invalid continuation.
        assert_eq!(d.decode(&[0xE2, 0x82]), "");
        assert_eq!(d.decode(&[0x41]), "\u{FFFD}A");
        assert_eq!(d.finish(), "");
    }

    #[test]
    fn round_trip() {
        for s in ["ASCII", "àçèéêëîïôùûüÿœæ", "ąčęłńšž", "€", "😀"] {
            assert_eq!(to_human_readable(&to_bytes(s)), s);
        }
    }
}
