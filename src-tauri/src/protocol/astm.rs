//! ASTM checksum computation and verification.

/// ASTM checksum: sum modulo 256 of the bytes following `STX`, up to and including `ETX` / `ETB`.
pub(crate) fn astm_checksum(content: &[u8]) -> u8 {
    content.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
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
