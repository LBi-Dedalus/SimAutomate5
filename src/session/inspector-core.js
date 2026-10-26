// Message inspector helpers. Pure module (no DOM access).
//
// Parsing is best effort. HL7 is parsed per event (one event per TCP read, so a message can
// be split or coalesced). ASTM units are grouped into one transfer per message beforehand
// (astm-group.js) and parsed as a whole.

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

/** Human readable names of ASTM E1394 records (first letter of each line). */
export const ASTM_RECORD_NAMES = {
  H: "Message header",
  P: "Patient",
  O: "Order",
  R: "Result",
  C: "Comment",
  Q: "Request information",
  M: "Manufacturer information",
  S: "Scientific",
  L: "Message terminator",
};

// Keys follow the E1394 numbering where field 1 is the record type letter.
const ASTM_FIELD_LABELS = {
  H: {
    2: "Delimiters",
    3: "Message control ID",
    4: "Access password",
    5: "Sender name",
    6: "Sender street address",
    8: "Sender telephone",
    9: "Sender characteristics",
    10: "Receiver ID",
    11: "Comments",
    12: "Processing ID",
    13: "Version",
    14: "Date/time",
  },
  P: {
    2: "Sequence number",
    3: "Practice patient ID",
    4: "Laboratory patient ID",
    5: "Patient ID (3)",
    6: "Patient name",
    7: "Mother's maiden name",
    8: "Birthdate",
    9: "Sex",
    10: "Race",
    11: "Address",
    13: "Telephone",
    14: "Attending physician",
  },
  O: {
    2: "Sequence number",
    3: "Specimen ID",
    4: "Instrument specimen ID",
    5: "Universal test ID",
    6: "Priority",
    7: "Requested date/time",
    8: "Collection date/time",
    9: "Collection end time",
    10: "Collection volume",
    11: "Collector ID",
    12: "Action code",
    13: "Danger code",
    14: "Relevant clinical info",
    15: "Received date/time",
    16: "Specimen descriptor",
    17: "Ordering physician",
    26: "Report type",
  },
  R: {
    2: "Sequence number",
    3: "Universal test ID",
    4: "Data value",
    5: "Units",
    6: "Reference ranges",
    7: "Abnormal flags",
    8: "Nature of abnormality testing",
    9: "Result status",
    10: "Date of change",
    11: "Operator ID",
    12: "Test started",
    13: "Test completed",
    14: "Instrument ID",
  },
  C: { 2: "Sequence number", 3: "Comment source", 4: "Comment text", 5: "Comment type" },
  Q: {
    2: "Sequence number",
    3: "Starting range ID",
    4: "Ending range ID",
    5: "Universal test ID",
    6: "Request time limits",
    7: "Results start date",
    8: "Results end date",
    9: "Requesting physician",
    10: "Physician telephone",
    13: "Request status",
  },
  L: { 2: "Sequence number", 3: "Termination code" },
};

const ASTM_CONTROL_TOKENS = /<(?:STX|ETX|ETB|ENQ|ACK|NAK|EOT)>/g;
const ASTM_LINE_BREAK = /<CR>|<LF>|\r\n|\r|\n/;
// eslint-disable-next-line no-control-regex
const ASTM_BARE_CONTROL = /[\u0002\u0003\u0004\u0005\u0006\u0015\u0017]/g;

/**
 * Splits the text of ASTM frames (control characters, frame number and checksum already
 * removed, or still shown as tokens) into records and fields, like parseHl7 does for HL7.
 * The field delimiter comes from the H record (default `|`, or `fallbackDelimiter`).
 */
export function parseAstm(text, fallbackDelimiter = "|") {
  const lines = String(text)
    .replace(ASTM_CONTROL_TOKENS, "")
    .replace(ASTM_BARE_CONTROL, "")
    .split(ASTM_LINE_BREAK)
    .filter((line) => line.trim() !== "");
  if (lines.length === 0) return null;

  const header = lines.find((line) => /^H[^A-Za-z0-9\s]/.test(line));
  const delimiter = header ? header.charAt(1) : fallbackDelimiter;

  const segments = lines.map((line) => {
    const name = line.charAt(0);
    if (!/[A-Z]/.test(name) || (line.length > 1 && line.charAt(1) !== delimiter)) {
      return { name: null, raw: line, fields: [] };
    }
    const fields = [];
    line
      .split(delimiter)
      .slice(1)
      .forEach((value, i) => {
        const index = i + 2;
        if (value !== "") {
          fields.push({
            id: `${name}-${index}`,
            label: ASTM_FIELD_LABELS[name]?.[index] ?? "",
            value,
          });
        }
      });
    return { name, raw: line, fields };
  });

  const field = (id) =>
    segments.flatMap((segment) => segment.fields).find((f) => f.id === id)?.value ?? "";
  return { segments, delimiter, sender: field("H-5"), version: field("H-13") };
}

/**
 * Parses the frames of an ASTM transfer: the acknowledged ones are joined and parsed as a
 * whole; the frames that were NAKed or left unanswered are parsed one by one.
 */
export function parseAstmTransfer(partition) {
  const merged = parseAstm(partition.merged.map((entry) => entry.frame.text).join(""));
  const delimiter = merged?.delimiter ?? "|";
  const apart = partition.apart.map((entry) => ({
    entry,
    parsed: parseAstm(entry.frame.text, delimiter),
  }));
  return { merged, apart };
}

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
  if (record.transfer) {
    return { kind: "astm", title: "ASTM message", hl7: null, transfer: record.transfer };
  }
  const hl7 = parseHl7(record.content);
  if (hl7) {
    return { kind: "hl7", title: hl7.messageType || "HL7", hl7 };
  }
  const starter = ASTM_STARTERS.find((token) => record.content.startsWith(token));
  if (!starter && record.content.startsWith("H|")) {
    return { kind: "astm", title: "ASTM frame", hl7: null };
  }
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
