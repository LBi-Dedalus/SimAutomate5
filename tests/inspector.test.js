import test from "node:test";
import assert from "node:assert/strict";

import {
  stripHl7Framing,
  parseHl7,
  describeMessage,
  toHexDump,
  hasRawBytes,
} from "../src/inspector-core.js";

const HL7 =
  "<VT>MSH|^~\\&|App|Fac|Rcv|Dst|20250722084438||ADR^A19|1753166763|P|2.4||AL|NE|<CR>" +
  "MSA|AE|1753166763|Patient not found||<CR><FS><CR>";

test("stripHl7Framing removes <VT> and <FS><CR>", () => {
  assert.equal(stripHl7Framing("<VT>MSH|x<CR><FS><CR>"), "MSH|x<CR>");
  assert.equal(stripHl7Framing("MSH|x<CR>"), "MSH|x<CR>");
});

test("parseHl7 numbers MSH fields with MSH-1 as the separator", () => {
  const parsed = parseHl7(HL7);
  assert.equal(parsed.messageType, "ADR^A19");
  assert.equal(parsed.controlId, "1753166763");
  assert.equal(parsed.version, "2.4");
  const msh = parsed.segments[0];
  assert.equal(msh.name, "MSH");
  assert.deepEqual(msh.fields.slice(0, 3).map((f) => [f.id, f.value]), [
    ["MSH-1", "|"],
    ["MSH-2", "^~\\&"],
    ["MSH-3", "App"],
  ]);
  const msa = parsed.segments[1];
  assert.deepEqual(msa.fields.map((f) => [f.id, f.value]), [
    ["MSA-1", "AE"],
    ["MSA-2", "1753166763"],
    ["MSA-3", "Patient not found"],
  ]);
  assert.equal(parsed.segments.length, 2);
});

test("parseHl7 rejects non HL7 content and invalid separators", () => {
  assert.equal(parseHl7("<ENQ>"), null);
  assert.equal(parseHl7("<STX>1H|\\^&|||<CR><ETX>00<CR><LF>"), null);
  assert.equal(parseHl7("MSHx"), null);
  assert.equal(parseHl7("PID|1"), null);
});

test("parseHl7 is best effort on fragments without a header", () => {
  assert.equal(parseHl7("MSA|AA|1<CR>"), null);
});

test("describeMessage classifies events", () => {
  assert.equal(describeMessage({ msg_type: "received", content: HL7 }).title, "ADR^A19");
  assert.equal(describeMessage({ msg_type: "sent", content: "<ENQ>" }).title, "ENQ");
  assert.equal(describeMessage({ msg_type: "sent", content: "<STX>1H|<CR><ETX>1A<CR><LF>" }).kind, "astm");
  assert.equal(describeMessage({ msg_type: "received", content: "hello" }).kind, "text");
  assert.equal(describeMessage({ msg_type: "systemerror", content: "boom" }).kind, "system");
});

test("toHexDump prints exact bytes with offsets and an ASCII column", () => {
  const bytes = [0x0b, ...Array.from("MSH|", (c) => c.charCodeAt(0)), 0x0d, 0x1c, 0x0d];
  const dump = toHexDump(bytes);
  assert.equal(dump, "0000  0B 4D 53 48 7C 0D 1C 0D                          .MSH|...");
  const long = toHexDump(new Array(17).fill(0x41));
  assert.equal(long.split("\n").length, 2);
  assert.ok(long.split("\n")[1].startsWith("0010  41"));
});

test("hasRawBytes only accepts non empty byte arrays", () => {
  assert.equal(hasRawBytes({ raw: [1] }), true);
  assert.equal(hasRawBytes({ raw: [] }), false);
  assert.equal(hasRawBytes({}), false);
});
