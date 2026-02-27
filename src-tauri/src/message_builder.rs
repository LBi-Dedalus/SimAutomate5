use anyhow::{anyhow, Result};

use crate::{
    models::{AutoBuildRequest, BuildRequest, BuildResponse, Protocol},
    translate::ControlToken,
};

pub fn build(req: BuildRequest) -> Result<BuildResponse> {
    match req.protocol {
        Protocol::Mllp => Ok(BuildResponse {
            output: build_mllp(&req.input),
        }),
        Protocol::Astm => Ok(BuildResponse {
            output: build_astm(&req.input),
        }),
        Protocol::Hl7 => Err(anyhow!("Use MLLP or ASTM for message builder")),
    }
}

pub fn auto_build(req: AutoBuildRequest) -> Result<BuildResponse> {
    let trimmed = req.input.trim_start();
    if trimmed.starts_with("H|") {
        return Ok(BuildResponse {
            output: build_astm(&req.input),
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
    let mut output = String::from("<VT>");
    for line in input.lines() {
        output.push_str(line);
        output.push_str("<CR>");
    }
    output.push_str("<FS><CR>");
    output
}

fn build_astm(input: &str) -> String {
    let lines = input.lines().filter(|l| !l.is_empty());
    let line_count = lines.clone().count();

    let segments: Vec<String> = lines
        .enumerate()
        .map(|(idx, l)| build_astm_segment(l, idx, line_count))
        .collect();

    segments.join("\n")
}

fn build_astm_segment(line: &str, idx: usize, line_count: usize) -> String {
    let segment_no = (idx + 1) % 8;
    let mut body = format!("{}{}", segment_no, line);

    body.push(ControlToken::CR.into());
    body.push(if idx == line_count - 1 {
        ControlToken::ETB.into()
    } else {
        ControlToken::ETX.into()
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
