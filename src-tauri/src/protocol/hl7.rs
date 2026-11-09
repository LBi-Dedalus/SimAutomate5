//! Minimal HL7 parsing and field access for trigger / condition matching and replies.

pub(crate) struct Hl7Message {
    field_sep: char,
    component_sep: char,
    segments: Vec<String>,
    message_type: String,
    /// MSH-10 exactly as received (the decoded `segments` are lossy for non UTF-8 bytes).
    raw_control_id: Option<Vec<u8>>,
}

/// MSH-10 taken straight from the received MSH segment bytes, only when non empty and free of
/// control characters.
fn raw_control_id(msh: &[u8], field_sep: u8) -> Option<Vec<u8>> {
    let id = msh.split(|b| *b == field_sep).nth(9)?;
    // The byte check is enough: C1 controls encoded in UTF-8 are not expected.
    if id.is_empty() || id.iter().any(|b| *b < 0x20 || *b == 0x7f) {
        return None;
    }
    Some(id.to_vec())
}

fn message_type(msh: &str, field_sep: char, component_sep: char) -> String {
    let message_type_field = msh.split(field_sep).nth(9);

    match message_type_field {
        None => return String::new(),
        Some(field) => field.split(component_sep).take(2).collect::<Vec<_>>().join("^"),
    }
}

pub(crate) fn parse_hl7(body: &[u8]) -> Option<Hl7Message> {
    let text = String::from_utf8_lossy(body);
    let segments: Vec<String> = text
        .split(['\r', '\n'])
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();
    let msh = segments.first()?;
    if !msh.starts_with("MSH") {
        return None;
    }
    let field_sep = msh.chars().nth(3)?;
    if !field_sep.is_ascii() || field_sep.is_control() || field_sep.is_alphanumeric() {
        return None;
    }
    // First encoding character, right after the field separator.
    let component_sep = msh
        .chars()
        .nth(4)
        .filter(|c| *c != field_sep)
        .unwrap_or('^');
    // `segments` are lossy for non UTF-8 bytes, so MSH-10 is read from the raw MSH segment.
    let raw_msh = body
        .split(|b| *b == b'\r' || *b == b'\n')
        .find(|segment| !segment.is_empty())?;
    Some(Hl7Message {
        field_sep,
        component_sep,
        raw_control_id: raw_control_id(raw_msh, field_sep as u8),
        message_type: message_type(msh, field_sep, component_sep),
        segments,
    })
}

impl Hl7Message {
    /// Field `n` of the first segment named `segment`. MSH-1 is the field separator itself,
    /// so MSH-n is the n-th separated item (MSH-2 are the encoding characters).
    pub(crate) fn field(&self, segment: &str, n: u32) -> Option<&str> {
        let n = n as usize;
        for line in &self.segments {
            let mut parts = line.split(self.field_sep);
            if parts.next() != Some(segment) {
                continue;
            }
            // The segment name is consumed, so `parts` now yields field 1 first. MSH-1 is the
            // separator itself, hence MSH-n is the (n-1)-th remaining item.
            if segment == "MSH" {
                if n == 1 {
                    return line.get(3..4);
                }
                return parts.nth(n - 2);
            }
            return parts.nth(n - 1);
        }
        None
    }

    /// First two MSH-9 components joined with `^` (`ORU^R01^ORU_R01` gives `ORU^R01`).
    pub(crate) fn message_type(&self) -> &str {
        &self.message_type
    }

    /// MSH-10 as received (raw bytes), only when non empty and free of control bytes.
    pub(crate) fn control_id(&self) -> Option<&[u8]> {
        self.raw_control_id.as_deref()
    }
}
