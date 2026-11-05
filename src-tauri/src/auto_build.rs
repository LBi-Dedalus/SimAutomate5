use crate::{
    protocol::astm::AstmEndingMode,
    translate::{
        ControlToken::{self, ENQ, EOT},
        ControlTokenDecoder,
    },
};

pub fn auto_build(input: String, astm_ending: AstmEndingMode) -> String {
    let trimmed = input.trim_start_matches(|c| char::is_ascii_whitespace(&c));
    if trimmed.starts_with("H|") {
        return build_astm(&input, astm_ending);
    }

    if trimmed.starts_with("MSH|") {
        return build_mllp(&input);
    }

    return input;
}

pub fn auto_build_from_raw(frames: Vec<Vec<u8>>, astm_ending: AstmEndingMode) -> Vec<Vec<u8>> {
    let Some(first_line) = frames.first() else {
        return frames;
    };

    let decoded_first_line = String::from_utf8_lossy(first_line);
    let trimmed_first_line =
        decoded_first_line.trim_start_matches(|c| char::is_ascii_whitespace(&c));

    if trimmed_first_line.starts_with("H|") {
        let autobuilt = build_astm_raw(&frames, astm_ending);
        return autobuilt;
    }

    if trimmed_first_line.starts_with("MSH|") {
        let autobuilt = build_mllp_raw(&frames);
        return vec![autobuilt.concat()];
    }

    return frames;
}

fn build_mllp(input: &str) -> String {
    let lines = input.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>();
    let line_count = lines.len();

    let mut built_lines = Vec::with_capacity(line_count + 2);
    built_lines.push("<VT>".to_string());

    for line in lines {
        let mut output = String::new();

        output.push_str(line);
        output.push(ControlToken::CR.char());

        built_lines.push(ControlTokenDecoder::decode_all(output.as_bytes()));
    }

    built_lines.push("<FS><CR>".to_string());

    built_lines.join("\n")
}

fn build_mllp_raw(input: &Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut built_lines = Vec::with_capacity(input.len() + 2);
    built_lines.push(vec![ControlToken::VT.byte()]);

    for line in input {
        let line = line.trim_ascii();
        if line.is_empty() {
            continue;
        }
        let mut output = vec![];

        output.extend(line);
        output.push(ControlToken::CR.byte());

        built_lines.push(output);
    }

    built_lines.push(vec![ControlToken::FS.byte(), ControlToken::CR.byte()]);
    built_lines
}

fn build_astm(input: &str, ending: AstmEndingMode) -> String {
    let lines = input.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>();
    let line_count = lines.len();

    let mut output = Vec::with_capacity(line_count + 2);
    output.push("<ENQ>".to_string());

    for (idx, line) in lines.iter().enumerate() {
        let segment = build_astm_segment(line.as_bytes(), ending, idx, line_count);
        output.push(ControlTokenDecoder::decode_all(&segment));
    }

    output.push("<EOT>".to_string());

    output.join("\n")
}

fn build_astm_raw(frames: &Vec<Vec<u8>>, astm_ending: AstmEndingMode) -> Vec<Vec<u8>> {
    let lines = frames
        .iter()
        .map(|f| f.trim_ascii())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>();
    let line_count = lines.len();

    let mut output = Vec::with_capacity(line_count + 2);
    output.push(vec![ENQ.byte()]);

    for (idx, frame) in lines.iter().enumerate() {
        let segment = build_astm_segment(frame, astm_ending, idx, line_count);
        output.push(segment);
    }

    output.push(vec![EOT.byte()]);
    output
}

fn build_astm_segment(
    line: &[u8],
    ending: AstmEndingMode,
    idx: usize,
    line_count: usize,
) -> Vec<u8> {
    let segment_no = (idx + 1) % 8;
    let mut body: Vec<u8> = vec![(('0' as u8) + (segment_no as u8))];
    body.extend(line);

    body.push(ControlToken::CR.byte());
    let is_last = idx == line_count - 1;
    body.push(match ending {
        AstmEndingMode::EtxForEveryRecord => ControlToken::ETX.byte(),
        AstmEndingMode::EtbBetweenRecords if is_last => ControlToken::ETX.byte(),
        AstmEndingMode::EtbBetweenRecords => ControlToken::ETB.byte(),
    });

    // `body.as_bytes()` are the UTF-8 wire bytes, so the checksum matches what is sent.
    let checksum = astm_checksum(&body);

    let mut output = Vec::new();
    output.push(ControlToken::STX.byte());
    output.extend(&body);
    output.extend(checksum.as_bytes());
    output.push(ControlToken::CR.byte());
    output.push(ControlToken::LF.byte());

    output
}

fn astm_checksum(body: &[u8]) -> String {
    format!("{:02X}", crate::protocol::astm::astm_checksum(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AutoBuildRequest;
    use crate::protocol::astm::AstmEndingMode;
    use crate::translate::ControlTokenEncoder;
    fn to_bytes(s: &str) -> Vec<u8> {
        ControlTokenEncoder::encode_all(s)
    }

    fn build(input: &str) -> String {
        build_with(input, AstmEndingMode::EtbBetweenRecords)
    }

    fn build_with(input: &str, astm_ending: AstmEndingMode) -> String {
        auto_build(AutoBuildRequest {
            input: input.to_string(),
            astm_ending,
        })
        .unwrap()
        .output
    }

    #[test]
    fn astm_ending_mode_selects_etb_or_etx_between_records() {
        let input = "H|a\nP|b\nL|c";
        let etb = build_with(input, AstmEndingMode::EtbBetweenRecords);
        assert_eq!(etb.matches("<ETB>").count(), 2);
        assert_eq!(etb.matches("<ETX>").count(), 1);
        let etx = build_with(input, AstmEndingMode::EtxForEveryRecord);
        assert_eq!(etx.matches("<ETB>").count(), 0);
        assert_eq!(etx.matches("<ETX>").count(), 3);
    }

    #[test]
    fn mllp_autobuild_is_utf8_and_matches_manual_tokens() {
        let output = build("MSH|^~\\&|S|F|R|F|2026||ADT^A01|1|P|2.4\rPID|1||Café");
        let bytes = to_bytes(&output);
        assert!(bytes.windows(2).any(|w| w == [0xC3, 0xA9]));
        assert!(!output.contains('Ã'));
        let manual = "<VT>\nMSH|^~\\&|S|F|R|F|2026||ADT^A01|1|P|2.4<CR>\nPID|1||Café<CR>\n<FS><CR>";
        // `\r` inside the input is not a line break for `lines()`, so build from LF.
        let built = build(&"MSH|^~\\&|S|F|R|F|2026||ADT^A01|1|P|2.4\nPID|1||Café".to_string());
        assert_eq!(to_bytes(&built), to_bytes(manual));
    }

    #[test]
    fn astm_autobuild_checksum_covers_utf8_bytes() {
        let output = build("H|\\^&|é");
        let bytes = to_bytes(&output);
        assert!(bytes.windows(2).any(|w| w == [0xC3, 0xA9]));
        let stx = bytes.iter().position(|b| *b == 0x02).unwrap();
        let end = bytes.iter().position(|b| *b == 0x03 || *b == 0x17).unwrap();
        let sum = bytes[stx + 1..=end]
            .iter()
            .fold(0u8, |a, b| a.wrapping_add(*b));
        let expected = format!("{:02X}", sum);
        assert_eq!(&bytes[end + 1..end + 3], expected.as_bytes());
    }
}
