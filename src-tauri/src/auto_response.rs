use chrono::Utc;

use crate::models::AutoResponseConfig;
use crate::translate::ControlToken::{ENQ, STX, VT};

/// Builds an automatic response message based on the provided configuration and incoming message.
/// The response is generated in human-readable format.
pub fn build_auto_response(cfg: &AutoResponseConfig, incoming: &[u8]) -> Option<String> {
    if !cfg.enabled {
        return None;
    }

    let first_char = incoming.first()?;

    if *first_char == STX as u8 || *first_char == ENQ as u8 {
        // ASTM message incoming
        return cfg.astm_message.as_ref().map(|msg| msg.clone());
    }
    if *first_char == VT as u8 {
        let msg_type = cfg.hl7_message_type.as_ref()?;
        let code = cfg.hl7_response_code.as_ref()?;
        let ack = generate_hl7_ack(incoming, msg_type, code)?;
        return Some(ack);
    }

    return None;
}

#[cfg(test)]
mod tests {
    use super::build_auto_response;
    use crate::models::AutoResponseConfig;
    use crate::translate::ControlToken;

    #[test]
    fn astm_no_response_for_ack_nak_eot() {
        let cfg = AutoResponseConfig {
            enabled: true,
            astm_message: Some("<ACK>".to_string()),
            hl7_message_type: None,
            hl7_response_code: None,
        };

        assert!(build_auto_response(&cfg, &[ControlToken::ACK as u8]).is_none());
        assert!(build_auto_response(&cfg, &[ControlToken::NAK as u8]).is_none());
        assert!(build_auto_response(&cfg, &[ControlToken::EOT as u8]).is_none());
    }

    #[test]
    fn astm_no_response_for_ack_with_crlf() {
        let cfg = AutoResponseConfig {
            enabled: true,
            astm_message: Some("<ACK>".to_string()),
            hl7_message_type: None,
            hl7_response_code: None,
        };

        assert!(build_auto_response(&cfg, &[ControlToken::ACK as u8, b'\r', b'\n']).is_none());
    }

    #[test]
    fn astm_response_still_sent_for_regular_payload() {
        let cfg = AutoResponseConfig {
            enabled: true,
            astm_message: Some("<ACK>".to_string()),
            hl7_message_type: None,
            hl7_response_code: None,
        };

        assert_eq!(
            build_auto_response(&cfg, b"\x021H|\\^&|...").as_deref(),
            Some("<ACK>")
        );
    }

    #[test]
    fn hl7_response_uses_configured_type_and_code() {
        let cfg = AutoResponseConfig {
            enabled: true,
            astm_message: None,
            hl7_message_type: Some("ACK^O21".to_string()),
            hl7_response_code: Some("AA".to_string()),
        };
        let incoming = b"\x0bMSH|^~\\&|REMOTE|LAB|SIMAUTO|SIM|20260825120000||OML^O21|CONTROL-123|P|2.5\rPID|1\r\x1c\r";

        let response = build_auto_response(&cfg, incoming).expect("an HL7 ACK response");

        assert!(response.starts_with("<VT>MSH|^~\\&|"));
        assert!(response.contains("||ACK^O21|"));
        assert!(response.contains("<CR>MSA|AA|CONTROL-123<CR>"));
        assert!(response.ends_with("<FS><CR>"));
    }

    #[test]
    fn hl7_response_preserves_incoming_control_id() {
        let cfg = AutoResponseConfig {
            enabled: true,
            astm_message: None,
            hl7_message_type: Some("ACK".to_string()),
            hl7_response_code: Some("AE".to_string()),
        };
        let incoming =
            b"\x0bMSH|^~\\&|REMOTE|LAB|SIMAUTO|SIM|20260825120000||ADT^A01|abc-987|P|2.5\r\x1c\r";

        let response = build_auto_response(&cfg, incoming).expect("an HL7 ACK response");

        assert!(response.contains("MSA|AE|abc-987<CR>"));
    }

    #[test]
    fn hl7_response_is_suppressed_when_disabled() {
        let cfg = AutoResponseConfig {
            enabled: false,
            astm_message: None,
            hl7_message_type: Some("ACK".to_string()),
            hl7_response_code: Some("AA".to_string()),
        };
        let incoming = b"\x0bMSH|^~\\&|REMOTE|LAB|SIMAUTO|SIM|20260825120000||ADT^A01|CONTROL-123|P|2.5\r\x1c\r";

        assert!(build_auto_response(&cfg, incoming).is_none());
    }
}

fn generate_hl7_ack(incoming: &[u8], msg_type: &str, code: &str) -> Option<String> {
    let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let control_id = extract_control_id(incoming)?;
    let new_id = Utc::now().format("%s%f").to_string();

    let ack = format!(
        "MSH|^~\\&|SIMAUTO|SIM|REMOTE|REMOTE|{timestamp}||{msg_type}|{new_id}|P|2.5\rMSA|{code}|{control_id}\r",
    );
    Some(format!("<VT>{}<FS><CR>", ack.replace("\r", "<CR>")))
}

fn extract_control_id(incoming: &[u8]) -> Option<String> {
    let msh = incoming
        .split(|byte| *byte == b'\r' || *byte == b'\n')
        .map(|segment| segment.strip_prefix(&[VT as u8]).unwrap_or(segment))
        .find(|segment| segment.starts_with(b"MSH"))?;
    let msh = str::from_utf8(msh).ok()?;
    let fields: Vec<&str> = msh.split('|').collect();
    fields.get(9).map(|f| f.to_string())
}
