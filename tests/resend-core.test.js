import test from "node:test";
import assert from "node:assert/strict";
import { encodeTokens, resendPayload } from "../src/session/resend-core.js";

const bytesOf = (content) => encodeTokens(content);
const hl7 = (content, raw = bytesOf(content)) => ({ msg_type: "sent", content, raw });
const HL7 = "<VT>MSH|^~\\&|A|B|C|D|20250101||ADT^A01|1|P|2.4<CR>PID|1<CR><FS><CR>";

const entry = (kind, content, extra = {}) => ({ kind, content, raw: bytesOf(content), frame: null, reply: null, ...extra });
const FRAME1 = "<STX>1H|\\^&|||<CR><ETX>00<CR><LF>";
const FRAME2 = "<STX>2L|1|N<CR><ETX>00<CR><LF>";
const transfer = (overrides = {}, entries) => ({
  msg_type: "sent",
  content: "",
  transfer: {
    direction: "sent",
    status: "complete",
    implicit: false,
    entries: entries ?? [
      entry("enq", "<ENQ>", { reply: { kind: "ack", content: "<ACK>" } }),
      entry("frame", FRAME1, { reply: { kind: "ack", content: "<ACK>" } }),
      entry("frame", FRAME2, { reply: { kind: "nak", content: "<NAK>" } }),
      entry("eot", "<EOT>"),
    ],
    ...overrides,
  },
});

test("encodeTokens mirrors the backend translator", () => {
  assert.deepEqual(encodeTokens("A<STX>B<x>"), [0x41, 2, 0x42, 0x3c, 0x78, 0x3e]);
  assert.deepEqual(encodeTokens("<<STX>"), [0x3c, 2]);
  assert.deepEqual(encodeTokens("<STX"), [0x3c, 0x53, 0x54, 0x58]);
  assert.deepEqual(encodeTokens("<NUL>>"), [0, 0x3e]);
  assert.deepEqual(encodeTokens("<é<STX>"), [0x3c, 0xc3, 0xa9, 2]);
  assert.deepEqual(encodeTokens("<😀>"), [...new TextEncoder().encode("<😀>")]);
});

test("HL7 framed from <VT> to <FS> (+ <CR>) is resendable with its own content", () => {
  assert.equal(resendPayload(hl7(HL7)), HL7);
  const noCr = HL7.replace(/<CR>$/, "");
  assert.equal(resendPayload(hl7(noCr)), noCr);
});

test("HL7 variants that are not a plain framed message are not resendable", () => {
  assert.equal(resendPayload(hl7("MSH|^~\\&|A<CR>")), null, "no <VT>");
  assert.equal(resendPayload(hl7("<VT>MSH|A<CR>")), null, "no <FS>");
  assert.equal(resendPayload(hl7("<VT>MSH|A<CR><FS><CR>PID|1")), null, "text after the frame");
  assert.equal(resendPayload(hl7("<VT>MSH|A<CR><FS><FS><CR>")), null, "two <FS>");
  assert.equal(resendPayload(hl7("<VT>MSH|A<CR><FS><CR><VT>MSH|B<CR><FS><CR>")), null, "two frames");
  assert.equal(resendPayload(hl7("<VT>MSH|A\n<FS>")), null, "real line break");
  assert.equal(resendPayload({ msg_type: "sent", content: HL7 }), null, "no raw bytes");
});

test("bare control records, received messages and notices are not resendable", () => {
  for (const content of ["<ACK>", "<NAK>", "<ENQ>", "<EOT>", "<STX>1H|<CR><ETX>00<CR><LF>", "hello"]) {
    assert.equal(resendPayload(hl7(content)), null, content);
  }
  assert.equal(resendPayload({ ...hl7(HL7), msg_type: "received" }), null);
  assert.equal(resendPayload({ ...hl7(HL7), msg_type: "systemerror" }), null);
  assert.equal(resendPayload(null), null);
});

test("content that cannot round-trip is not resendable", () => {
  assert.equal(resendPayload(hl7("<VT>MSH|caf\uFFFD<CR><FS><CR>")), null, "replacement char");
  // The wire carried the five ASCII characters <STX>, which would be re-encoded as a control byte.
  const ascii = (text) => [...new TextEncoder().encode(text)];
  const literal = "<VT>MSH|<STX><CR><FS><CR>";
  const literalRaw = [0x0b, ...ascii("MSH|<STX>"), 0x0d, 0x1c, 0x0d];
  assert.equal(resendPayload(hl7(literal, literalRaw)), null);
  // A literal "<" before a real token is encoded as the backend expects (<<STX> = "<" + STX).
  const lt = "<VT>MSH|<<STX><CR><FS><CR>";
  assert.equal(resendPayload(hl7(lt)), lt);
  // Raw that differs from the re-encoded content.
  assert.equal(resendPayload(hl7(HL7, [1, 2, 3])), null);
});

test("a complete sent ASTM transfer is rebuilt from its entries, without replies or extra lines", () => {
  assert.equal(resendPayload(transfer()), `<ENQ>\n${FRAME1}\n${FRAME2}\n<EOT>`);
});

test("ASTM transfers that are not complete sent ones starting with ENQ are not resendable", () => {
  assert.equal(resendPayload(transfer({ status: "open" })), null);
  assert.equal(resendPayload(transfer({ status: "incomplete" })), null);
  assert.equal(resendPayload(transfer({ direction: "received" })), null);
  assert.equal(resendPayload({ ...transfer(), msg_type: "received" }), null);
  const implicit = [entry("frame", FRAME1), entry("eot", "<EOT>")];
  assert.equal(resendPayload(transfer({ implicit: true }, implicit)), null);
  assert.equal(resendPayload(transfer({}, implicit)), null, "no ENQ");
  assert.equal(resendPayload(transfer({}, [entry("enq", "<ENQ>"), entry("frame", FRAME1)])), null, "no EOT");
  assert.equal(resendPayload(transfer({}, [entry("enq", "<ENQ>"), entry("eot", "<EOT>")])), "<ENQ>\n<EOT>");
});

test("ASTM entries with unfaithful text or odd content are not resendable", () => {
  const good = () => [entry("enq", "<ENQ>"), entry("frame", FRAME1), entry("eot", "<EOT>")];
  const withFrame = (frame) => [entry("enq", "<ENQ>"), frame, entry("eot", "<EOT>")];
  assert.equal(resendPayload(transfer({}, good())) === null, false);
  assert.equal(resendPayload(transfer({}, withFrame(entry("frame", "<STX>1H|\uFFFD<CR><ETX>00<CR><LF>")))), null);
  assert.equal(resendPayload(transfer({}, withFrame({ ...entry("frame", FRAME1), raw: [] }))), null, "no raw");
  assert.equal(resendPayload(transfer({}, withFrame({ ...entry("frame", FRAME1), raw: [1] }))), null);
  assert.equal(resendPayload(transfer({}, withFrame(entry("frame", "1H|no stx")))), null);
  assert.equal(resendPayload(transfer({}, withFrame(entry("frame", "<STX>1H|\n<ETX>00<CR><LF>")))), null, "line break");
  assert.equal(resendPayload(transfer({}, [entry("enq", "<ENQ>"), entry("enq", "<ENQ>"), entry("eot", "<EOT>")])), null);
});
