//! Translation of the composer text into the items written to the socket.

use crate::translate;

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

/// Splits the composer text into the items written to the socket.
///
/// An HL7/MLLP frame (`<VT>` ... `<FS>` plus the following `<CR>`, human-readable tokens or
/// raw bytes) is ONE item: the line breaks inside it are composer formatting and are not sent.
/// Leading whitespace before `<VT>` is skipped; an unterminated frame runs to the end of the
/// text. Everything else keeps the line-based behaviour (one item per line, newlines dropped),
/// which the ASTM ENQ/STX/ACK handling relies on.
///
/// `message` is considered "complete" (auto-built): the control tokens (`<VT>`, `<FS>`,
/// `<CR>`, `<ENQ>`, ...) are already present in the text. This function never adds any.
pub(super) fn split_message(message: &str) -> Vec<Vec<u8>> {
    let mut items = Vec::new();
    let mut rest = message;
    while !rest.is_empty() {
        let trimmed = rest.trim_start_matches(['\n', '\r', ' ', '\t']);
        let (item, next) = if starts_mllp(trimmed) {
            next_hl7_message(trimmed)
        } else {
            next_non_hl7_message(rest)
        };
        items.push(item);
        rest = next;
    }
    items
}

/// Extracts the next HL7/MLLP frame from `text` (which must start with `<VT>`) as a single
/// item, and returns it with the remaining text.
fn next_hl7_message(text: &str) -> (Vec<u8>, &str) {
    let end = match find_first(text, &["<FS>", "\u{1C}"]) {
        Some((index, len)) => {
            let mut end = index + len;
            let after = &text[end..];
            if after.starts_with("<CR>") {
                end += 4;
            } else if after.starts_with('\r') && !after.starts_with("\r\n") {
                end += 1;
            }
            end
        }
        None => text.len(),
    };
    // Each line is translated separately and the bytes are concatenated: the line breaks are
    // not sent and a token can never span two lines. A CR is dropped only when it precedes
    // an LF; a lone CR is data.
    let item = text[..end]
        .split("\r\n")
        .flat_map(|chunk| chunk.split('\n'))
        .flat_map(translate::ControlTokenEncoder::encode_all)
        .collect();
    let rest = &text[end..];
    // The line break that ends the frame's last line is formatting too.
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    (item, rest)
}

/// Extracts the next line of `text` (anything that is not an HL7/MLLP frame, e.g. ASTM) as
/// one item, and returns it with the remaining text.
fn next_non_hl7_message(text: &str) -> (Vec<u8>, &str) {
    // A CR is stripped only when it precedes the LF consumed here; a trailing CR
    // without LF is data (as with `str::lines()`).
    let (line, next) = match text.find('\n') {
        Some(index) => {
            let line = &text[..index];
            (line.strip_suffix('\r').unwrap_or(line), &text[index + 1..])
        }
        None => (text, ""),
    };
    (translate::ControlTokenEncoder::encode_all(line), next)
}
