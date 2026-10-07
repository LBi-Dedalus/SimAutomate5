// Message inspector helpers. Pure module (no DOM access).
//
// Parsing is best effort and PER EVENT: the backend emits one event per TCP read,
// so a protocol message can be split over several events or several messages can
// be coalesced in one. No full message framing is attempted here.

const HL7_FIELD_LABELS = {
  MSH: {
    1: "Field separator",
    2: "Encoding characters",
    3: "Sending application",
    4: "Sending facility",
    5: "Receiving application",
    6: "Receiving facility",
    7: "Date/time",
    9: "Message type",
    10: "Control ID",
    11: "Processing ID",
    12: "Version",
  },
  MSA: { 1: "Acknowledgement code", 2: "Control ID", 3: "Text message" },
  PID: {
    1: "Set ID",
    2: "Patient ID (external)",
    3: "Patient identifier list",
    5: "Patient name",
    7: "Date of birth",
    8: "Sex",
    11: "Address",
  },
  PV1: { 1: "Set ID", 2: "Patient class", 3: "Assigned location", 19: "Visit number" },
  QRD: {
    1: "Query date/time",
    2: "Query format",
    3: "Query priority",
    4: "Query ID",
    7: "Quantity limit",
    8: "Who subject filter",
    9: "What subject filter",
    10: "What department",
  },
  QRF: { 1: "Where subject filter", 2: "When start", 3: "When end" },
  ORC: { 1: "Order control", 2: "Placer order number", 3: "Filler order number", 5: "Order status" },
  OBR: {
    1: "Set ID",
    2: "Placer order number",
    3: "Filler order number",
    4: "Universal service ID",
    7: "Observation date/time",
  },
  OBX: {
    1: "Set ID",
    2: "Value type",
    3: "Observation identifier",
    5: "Observation value",
    6: "Units",
    7: "Reference range",
    8: "Abnormal flags",
    11: "Result status",
  },
  NTE: { 1: "Set ID", 2: "Source", 3: "Comment" },
  ERR: { 1: "Error location" },
  EVN: { 1: "Event type code", 2: "Recorded date/time" },
};

/** Human readable names of common HL7 v2 segments. */
export const HL7_SEGMENT_NAMES = {
  MSH: "Message header",
  MSA: "Acknowledgment",
  ERR: "Error",
  EVN: "Event type",
  PID: "Patient identification",
  PD1: "Patient additional demographic",
  NK1: "Next of kin",
  PV1: "Patient visit",
  PV2: "Patient visit (additional)",
  QRD: "Query definition",
  QRF: "Query filter",
  QAK: "Query acknowledgment",
  QPD: "Query parameter definition",
  RCP: "Response control parameter",
  ORC: "Common order",
  OBR: "Observation request",
  OBX: "Observation result",
  NTE: "Notes and comments",
  SPM: "Specimen",
  SAC: "Specimen container",
  TQ1: "Timing/quantity",
  DSP: "Display data",
  DSC: "Continuation pointer",
  INV: "Inventory detail",
  EQU: "Equipment detail",
};

/** Removes MLLP framing tokens (<VT> ... <FS><CR>) around an HL7 message. */
export function stripHl7Framing(content) {
  let body = String(content).trimStart();
  if (body.startsWith("<VT>")) body = body.slice(4);
  body = body.replace(/(?:<FS>(?:<CR>)?)\s*$/, "");
  return body;
}

/**
 * Parses an HL7 v2 message shown with control tokens. Returns null when the
 * content does not look like HL7 (no MSH segment with a valid field separator).
 */
export function parseHl7(content) {
  const body = stripHl7Framing(content);
  if (!body.startsWith("MSH")) return null;
  const separator = body.charAt(3);
  if (separator === "" || /[A-Za-z0-9<>\s]/.test(separator)) return null;

  const lines = body
    .split(/<CR>|<LF>|\r\n|\r|\n/)
    .filter((line) => line.trim() !== "");

  const segments = lines.map((line) => {
    const nameMatch = /^[A-Z][A-Z0-9]{2}/.exec(line);
    if (!nameMatch || (line.length > 3 && line.charAt(3) !== separator)) {
      return { name: null, raw: line, fields: [] };
    }
    const name = nameMatch[0];
    const parts = line.split(separator);
    const fields = [];
    if (name === "MSH") {
      fields.push({ id: "MSH-1", label: HL7_FIELD_LABELS.MSH[1], value: separator });
      parts.slice(1).forEach((value, i) => {
        const index = i + 2; // MSH-2 is the first element after the separator
        if (value !== "") {
          fields.push({
            id: `MSH-${index}`,
            label: HL7_FIELD_LABELS.MSH[index] ?? "",
            value,
          });
        }
      });
    } else {
      parts.slice(1).forEach((value, i) => {
        const index = i + 1;
        if (value !== "") {
          fields.push({
            id: `${name}-${index}`,
            label: HL7_FIELD_LABELS[name]?.[index] ?? "",
            value,
          });
        }
      });
    }
    return { name, raw: line, fields };
  });

  const msh = segments.find((segment) => segment.name === "MSH");
  const field = (id) => msh?.fields.find((f) => f.id === id)?.value ?? "";
  return {
    segments,
    messageType: field("MSH-9"),
    controlId: field("MSH-10"),
    version: field("MSH-12"),
  };
}

const ASTM_STARTERS = ["<STX>", "<ENQ>", "<EOT>", "<ACK>", "<NAK>"];

/** MSA-1 acknowledgment of a parsed HL7 message: { code, ok } or null. */
export function hl7Ack(hl7) {
  const msa = hl7?.segments.find((segment) => segment.name === "MSA");
  const code = msa?.fields.find((f) => f.id === "MSA-1")?.value ?? "";
  if (!code) return null;
  return { code, ok: code === "AA" || code === "CA" };
}

/**
 * Classifies a stream event. kind: system | hl7 | astm | text.
 * title is a short label for the bubble/inspector.
 */
export function describeMessage(record) {
  if (record.msg_type !== "sent" && record.msg_type !== "received") {
    return { kind: "system", title: systemTitle(record.msg_type), hl7: null };
  }
  const hl7 = parseHl7(record.content);
  if (hl7) {
    return { kind: "hl7", title: hl7.messageType || "HL7", hl7 };
  }
  const starter = ASTM_STARTERS.find((token) => record.content.startsWith(token));
  if (starter) {
    return {
      kind: "astm",
      title: starter === "<STX>" ? "ASTM frame" : starter.slice(1, -1),
      hl7: null,
    };
  }
  return { kind: "text", title: "Text", hl7: null };
}

function systemTitle(type) {
  switch (type) {
    case "systemwarn":
      return "Warning";
    case "systemerror":
      return "Error";
    default:
      return "Info";
  }
}

/** Hex dump of the exact wire bytes: offset, 16 hex bytes, ASCII column. */
export function toHexDump(bytes) {
  const lines = [];
  for (let offset = 0; offset < bytes.length; offset += 16) {
    const chunk = bytes.slice(offset, offset + 16);
    const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, "0").toUpperCase());
    const ascii = Array.from(chunk, (b) =>
      b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".",
    ).join("");
    lines.push(
      `${offset.toString(16).padStart(4, "0")}  ${hex.join(" ").padEnd(47, " ")}  ${ascii}`,
    );
  }
  return lines.join("\n");
}

export function hasRawBytes(record) {
  return Array.isArray(record.raw) && record.raw.length > 0;
}
