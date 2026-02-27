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

impl Into<char> for ControlToken {
    fn into(self) -> char {
        self as u8 as char
    }
}

pub fn to_bytes(input: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '<' && i + 1 < chars.len() {
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '>') {
                let token: String = chars[i..=i + end + 1].iter().collect();
                if let Some(byte) = token_to_byte(&token) {
                    output.push(byte);
                    i += end + 2;
                    continue;
                }
            }
        }

        output.push(chars[i] as u8);
        i += 1;
    }

    output
}

pub fn to_human_readable(bytes: &[u8]) -> String {
    let mut result = String::new();
    for byte in bytes {
        if let Some(token) = byte_to_token(*byte) {
            result.push_str(token);
        } else if *byte == b'\n' {
            result.push('\n');
        } else {
            result.push(*byte as char);
        }
    }
    result
}

fn token_to_byte(token: &str) -> Option<u8> {
    CONTROL_TOKENS
        .iter()
        .find(|(t, _)| *t == token)
        .map(|(_, b)| *b as u8)
}

fn byte_to_token(byte: u8) -> Option<&'static str> {
    CONTROL_TOKENS
        .iter()
        .find(|(_, b)| (*b as u8) == byte)
        .map(|(t, _)| *t)
}
