use anyhow::Result;

use crate::{
    models::{AutoBuildRequest, BuildResponse},
    translate::{escape_controls, ControlToken},
};

pub fn auto_build(req: AutoBuildRequest) -> Result<BuildResponse> {
    let trimmed = req.input.trim_start();
    if trimmed.starts_with("H|") {
        return Ok(BuildResponse {
            output: build_astm(&req.input, req.no_etb),
        });
    }

    if trimmed.starts_with("MSH|") {
        return Ok(BuildResponse {
            output: build_mllp(&req.input),
        });
    }

    Ok(BuildResponse { output: req.input })
}

fn build_mllp(input: &str) -> String {
    let lines = input.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>();
    let line_count = lines.len();

    let mut built_lines = Vec::with_capacity(line_count + 2);
    built_lines.push("<VT>".to_string());

    for line in lines {
        let mut output = String::new();

        output.push_str(line);
        output.push(ControlToken::CR.into());

        built_lines.push(escape_controls(&output));
    }

    built_lines.push("<FS><CR>".to_string());

    built_lines.join("\n")
}

fn build_astm(input: &str, no_etb: bool) -> String {
    let lines = input.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>();
    let line_count = lines.len();

    let mut output = Vec::with_capacity(line_count + 2);
    output.push("<ENQ>".to_string());

    for (idx, line) in lines.iter().enumerate() {
        let segment = build_astm_segment(line, no_etb, idx, line_count);
        output.push(escape_controls(&segment));
    }

    output.push("<EOT>".to_string());

    output.join("\n")
}

fn build_astm_segment(line: &str, no_etb: bool, idx: usize, line_count: usize) -> String {
    let segment_no = (idx + 1) % 8;
    let mut body = format!("{}{}", segment_no, line);

    body.push(ControlToken::CR.into());
    body.push(if no_etb || idx == line_count - 1 {
        ControlToken::ETX.into()
    } else {
        ControlToken::ETB.into()
    });

    // `body.as_bytes()` are the UTF-8 wire bytes, so the checksum matches what is sent.
    let checksum = astm_checksum(body.as_bytes());

    let mut output = String::new();
    output.push(ControlToken::STX.into());
    output.push_str(&body);
    output.push_str(&checksum);
    output.push(ControlToken::CR.into());
    output.push(ControlToken::LF.into());

    output
}

fn astm_checksum(body: &[u8]) -> String {
    let mut sum: u8 = 0;
    for byte in body {
        sum = sum.wrapping_add(*byte);
    }
    format!("{:02X}", sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AutoBuildRequest;
    use crate::translate::to_bytes;

    fn build(input: &str) -> String {
        auto_build(AutoBuildRequest {
            input: input.to_string(),
            no_etb: false,
        })
        .unwrap()
        .output
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
