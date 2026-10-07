// Control characters shared by the composer buttons, the template editor and the inspector.
// Pure module (no DOM access) so it can be unit tested with node:test.

export const CONTROL_NAMES = [
  "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL",
  "BS", "HT", "LF", "VT", "FF", "CR", "SO", "SI",
  "DLE", "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB",
  "CAN", "EM", "SUB", "ESC", "FS", "GS", "RS", "US",
];

const CONTROL_SET = new Set(CONTROL_NAMES);

/**
 * Converts a button `data-token` such as "VT" or "FS.CR" into the textual payload
 * understood by the backend ("<VT>" / "<FS><CR>"). Returns null for unknown names.
 */
export function tokenPayload(dataToken) {
  if (typeof dataToken !== "string" || dataToken.trim() === "") return null;
  const names = dataToken.split(".").map((name) => name.trim().toUpperCase());
  if (!names.every((name) => CONTROL_SET.has(name))) return null;
  return names.map((name) => `<${name}>`).join("");
}

/** Splits text into plain text and `<XXX>` control token parts. */
export function splitTokens(content) {
  const parts = [];
  const re = /<([A-Z0-9]{2,3})>/g;
  let last = 0;
  let match;
  while ((match = re.exec(content)) !== null) {
    if (!CONTROL_SET.has(match[1])) continue;
    if (match.index > last) {
      parts.push({ type: "text", value: content.slice(last, match.index) });
    }
    parts.push({ type: "ctrl", value: match[1] });
    last = match.index + match[0].length;
  }
  if (last < content.length) {
    parts.push({ type: "text", value: content.slice(last) });
  }
  return parts;
}
