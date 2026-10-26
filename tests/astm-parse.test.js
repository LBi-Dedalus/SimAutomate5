import test from "node:test";
import assert from "node:assert/strict";
import { parseAstm, parseAstmTransfer, describeMessage } from "../src/session/inspector-core.js";
import { partitionFrames } from "../src/session/astm-group.js";

test("records and fields are split on the field delimiter, empty fields skipped", () => {
  const parsed = parseAstm("H|\\^&|||Sender|||||Rcv||P|1\rP|1||PID123||Doe^John\rL|1|N\r");
  assert.deepEqual(
    parsed.segments.map((segment) => segment.name),
    ["H", "P", "L"],
  );
  assert.equal(parsed.delimiter, "|");
  assert.equal(parsed.sender, "Sender");
  const patient = parsed.segments[1];
  assert.deepEqual(
    patient.fields.map((f) => [f.id, f.value, f.label]),
    [
      ["P-2", "1", "Sequence number"],
      ["P-4", "PID123", "Laboratory patient ID"],
      ["P-6", "Doe^John", "Patient name"],
    ],
  );
});

test("the delimiter comes from the H record", () => {
  const parsed = parseAstm("H;\\^&;;;Sender\rR;1;GLU;5.2\rL;1\r");
  assert.equal(parsed.delimiter, ";");
  assert.deepEqual(
    parsed.segments[1].fields.map((f) => [f.id, f.value]),
    [["R-2", "1"], ["R-3", "GLU"], ["R-4", "5.2"]],
  );
});

test("control tokens and line breaks of any kind are ignored", () => {
  const parsed = parseAstm("<STX>H|\\^&<CR>P|1<CR><LF>R|1|X\r\nL|1<ETX>");
  assert.deepEqual(
    parsed.segments.map((segment) => segment.raw),
    ["H|\\^&", "P|1", "R|1|X", "L|1"],
  );
});

test("a line that is not a record is kept as unparsed", () => {
  const parsed = parseAstm("H|\\^&\rgarbage\r");
  assert.equal(parsed.segments[1].name, null);
  assert.equal(parsed.segments[1].raw, "garbage");
});

test("nothing to parse gives null", () => {
  assert.equal(parseAstm("<STX><ETX>"), null);
});

const frame = (number, text, terminator, reply) => ({
  kind: "frame",
  timestamp: "T",
  content: "",
  raw: [],
  frame: { number, text, terminator, checksum_ok: true },
  reply: reply ? { kind: reply } : null,
});

test("a record continued over an ETB frame is joined before parsing", () => {
  const transfer = {
    status: "complete",
    entries: [
      frame(1, "H|\\^&\rR|1|GLU|5", "ETB", "ack"),
      frame(2, ".2|mmol/L\rL|1\r", "ETX", "ack"),
    ],
  };
  const { merged, apart } = parseAstmTransfer(partitionFrames(transfer));
  assert.equal(apart.length, 0);
  const result = merged.segments.find((segment) => segment.name === "R");
  assert.deepEqual(
    result.fields.map((f) => f.value),
    ["1", "GLU", "5.2", "mmol/L"],
  );
});

test("unacknowledged frames are parsed on their own with the delimiter of the message", () => {
  const transfer = {
    status: "complete",
    entries: [
      frame(1, "H;\\^&\r", "ETB", "ack"),
      frame(2, "P;1;;PID\r", "ETB", "nak"),
      frame(3, "L;1\r", "ETX", "ack"),
    ],
  };
  const { merged, apart } = parseAstmTransfer(partitionFrames(transfer));
  assert.equal(merged.segments.length, 2);
  assert.equal(apart.length, 1);
  assert.equal(apart[0].parsed.segments[0].fields[1].value, "PID");
});

test("a transfer record is described as one ASTM message", () => {
  const info = describeMessage({
    msg_type: "received",
    content: "<ENQ>",
    transfer: { entries: [] },
  });
  assert.equal(info.kind, "astm");
  assert.equal(info.title, "ASTM message");
  assert.ok(info.transfer);
});
