use chrono::Utc;

use crate::models::AutoResponseConfig;
use crate::translate::ControlToken::{self, ENQ, STX, VT};

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
            build_auto_response(&cfg, b"<STX>1H|\\^&|...").as_deref(),
            Some("<ACK>")
        );
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
    let text = String::from_utf8_lossy(incoming).to_string();
    let bytes = text
        .bytes()
        .filter(|b| *b <= (ControlToken::US as u8) || *b == b'\n')
        .collect::<Vec<u8>>();
    let res = unsafe { str::from_utf8_unchecked(&bytes) };

    let msh = res.lines().find(|line| line.starts_with("MSH"))?;
    let fields: Vec<&str> = msh.split('|').collect();
    fields.get(9).map(|f| f.to_string())
}
