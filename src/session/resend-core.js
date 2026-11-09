// Pure rules for resending a sent message from the inspector (no DOM, no Tauri).
//
// The backend send path takes a string: control tokens (<STX>...) become bytes, anything else is
// UTF-8, an HL7 frame (<VT> ... first <FS> [+ <CR>]) is one item, and any other text is split
// into one item per line (see composer.rs / translate.rs). A message is only resent when the text
// rebuilt from the stored history encodes to exactly the bytes that were recorded; otherwise the
// stored text is not faithful and Resend stays disabled (nothing is guessed).

const TOKEN_NAMES = [
  "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "HT", "LF", "VT", "FF", "CR", "SO", "SI",
  "DLE", "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC", "FS", "GS", "RS", "US",
];
const TOKEN_BYTES = new Map(TOKEN_NAMES.map((name, byte) => [`<${name}>`, byte]));
const MAX_TOKEN_LEN = 5;

/** Mirror of translate.rs ControlTokenEncoder::encode_all. */
export function encodeTokens(text) {
  const out = [];
  const utf8 = new TextEncoder();
  let plain = "";
  const flush = () => {
    if (plain) out.push(...utf8.encode(plain));
    plain = "";
  };
  let i = 0;
  while (i < text.length) {
    if (text[i] === "<") {
      const end = text.slice(i, i + MAX_TOKEN_LEN).indexOf(">");
      if (end !== -1) {
        const byte = TOKEN_BYTES.get(text.slice(i, i + end + 1));
        if (byte !== undefined) {
          flush();
          out.push(byte);
          i += end + 1;
          continue;
        }
      }
    }
    plain += text[i];
    i += 1;
  }
  flush();
  return out;
}

const sameBytes = (a, b) => a.length === b.length && a.every((byte, index) => byte === b[index]);

/** True when `content` is faithful text for `raw`: no replacement char and the same bytes back. */
function roundTrips(content, raw) {
  if (typeof content !== "string" || !Array.isArray(raw) || raw.length === 0) return false;
  if (content.includes("\uFFFD")) return false;
  return sameBytes(encodeTokens(content), raw);
}

const hasLineBreak = (text) => /[\r\n]/.test(text);
const countOf = (text, part) => text.split(part).length - 1;

function hl7Payload(record) {
  const { content } = record;
  if (typeof content !== "string" || !content.startsWith("<VT>")) return null;
  if (!/<FS>(<CR>)?$/.test(content) || countOf(content, "<FS>") !== 1) return null;
  if (hasLineBreak(content)) return null;
  return roundTrips(content, record.raw) ? content : null;
}

function astmPayload(record) {
  const { transfer } = record;
  if (transfer.direction !== "sent" || transfer.status !== "complete" || transfer.implicit) return null;
  const entries = transfer.entries;
  if (!Array.isArray(entries) || entries.length < 2) return null;
  if (entries[0].kind !== "enq" || entries.at(-1).kind !== "eot") return null;
  for (const [index, entry] of entries.entries()) {
    const middle = index > 0 && index < entries.length - 1;
    if (middle ? entry.kind !== "frame" : entry.kind === "frame") return null;
    const { content } = entry;
    if (typeof content !== "string" || content === "" || hasLineBreak(content)) return null;
    if (entry.kind === "enq" && content !== "<ENQ>") return null;
    if (entry.kind === "eot" && content !== "<EOT>") return null;
    if (entry.kind === "frame" && !content.startsWith("<STX>")) return null;
    if (!roundTrips(content, entry.raw)) return null;
  }
  // One item per line for the backend; replies (entry.reply) are never part of the payload.
  return entries.map((entry) => entry.content).join("\n");
}

/** The string to send to resend `record`, or null when it cannot be resent faithfully. */
export function resendPayload(record) {
  if (!record || record.msg_type !== "sent") return null;
  if (record.transfer) return astmPayload(record);
  return hl7Payload(record);
}

export const RESEND_HINT = "Send this message again to its session";
export const RESEND_UNAVAILABLE_HINT = "Only complete sent HL7 or ASTM messages can be resent";
export const RESEND_OFFLINE_HINT = "The session of this message cannot send now";
