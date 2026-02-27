use chrono::Utc;

use crate::models::{AutoResponseConfig, Protocol};
use crate::translate::{self, ControlToken};

pub fn build_auto_response(
    protocol: &Protocol,
    cfg: &AutoResponseConfig,
    incoming: &[u8],
) -> Option<Vec<u8>> {
    if !cfg.enabled {
        return None;
    }

    match protocol {
        Protocol::Astm => cfg
            .astm_message
            .as_ref()
            .filter(|_| cfg.enabled)
            .map(|msg| translate::to_bytes(msg)),
        Protocol::Hl7 => {
            let msg_type = cfg.hl7_message_type.as_ref()?;
            let code = cfg.hl7_response_code.as_ref()?;
            let ack = generate_hl7_ack(incoming, msg_type, code);
            Some(translate::to_bytes(&ack))
        }
    }
}

fn generate_hl7_ack(incoming: &[u8], msg_type: &str, code: &str) -> String {
    let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let control_id = extract_control_id(incoming).unwrap_or("1".to_string());
    let new_id = Utc::now().format("%s%f").to_string();

    let ack = format!(
        "MSH|^~\\&|SIMAUTO|SIM|REMOTE|REMOTE|{timestamp}||{msg_type}|{new_id}|P|2.5\rMSA|{code}|{control_id}\r",
    );
    format!("<VT>{}<FS><CR>", ack.replace("\r", "<CR>"))
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
