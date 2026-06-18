use anyhow::Result;

use crate::{
    models::{AutoBuildRequest, BuildResponse},
    translate::{to_human_readable, ControlToken},
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

        built_lines.push(to_human_readable(output.as_bytes()));
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
        output.push(to_human_readable(segment.as_bytes()));
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
