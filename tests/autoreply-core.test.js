import test from "node:test";
import assert from "node:assert/strict";
import {
  describeAction,
  describeCondition,
  describeDelay,
  moveRule,
  newRule,
  previewTemplate,
  readLegacySettings,
  removeLegacySettings,
  ruleFromPersisted,
  ruleToPersisted,
  templateProblems,
  validateRule,
  validateRules,
} from "../src/autoreply-core.js";

const TPL = [
  { id: "adr", name: "ADR", payload: "MSH|^~\\&|X|{{NOW}}||ADR^A19|{{CONTROL_ID}}\rMSA|AA|{{REQ_CONTROL_ID}}", variables: [{ name: "REQ_CONTROL_ID", default: "" }] },
  { id: "bad", name: "Bad", payload: "A {{UNSET}}", variables: [{ name: "UNSET", default: "" }] },
  { id: "plain", name: "Plain", payload: "<ACK>", variables: [] },
];

const rule = (patch = {}) => ({ ...newRule("r1"), name: "R", messageType: "QRY^A19", templateId: "plain", ...patch });
const fields = (r, templates = TPL) => validateRule(r, templates).map((e) => e.field);

test("a valid rule has no problems and round-trips to the persisted shape", () => {
  const r = rule({ useCondition: true, condSegment: "QRD", condField: "8", condOperator: "glob", condValue: "AAZ*", delay: "50" });
  assert.deepEqual(validateRule(r, TPL), []);
  const stored = ruleToPersisted(r);
  assert.deepEqual(stored, {
    id: "r1",
    name: "R",
    enabled: true,
    trigger: { type: "hl7", message_type: "QRY^A19" },
    condition: { segment: "QRD", field: 8, operator: "glob", value: "AAZ*" },
    action: { type: "template", template_id: "plain" },
    delay_ms: 50,
  });
  assert.deepEqual(ruleToPersisted(ruleFromPersisted(stored)), stored);
});

test("ASTM triggers carry no message type or condition; literal and generated actions are stored in the rule", () => {
  const astm = rule({ trigger: "astm_enq", useCondition: true, condSegment: "X", action: "literal", literal: "<ACK>" });
  const stored = ruleToPersisted(astm);
  assert.deepEqual(stored.trigger, { type: "astm_enq" });
  assert.equal(stored.condition, null);
  assert.deepEqual(stored.action, { type: "literal", text: "<ACK>" });
  const ack = ruleToPersisted(rule({ action: "hl7_ack", ackType: "ACK^O21", ackCode: "AE" }));
  assert.deepEqual(ack.action, { type: "hl7_ack", message_type: "ACK^O21", code: "AE" });
});

test("No auto reply is stored explicitly, ignores irrelevant fields, and still validates the trigger", () => {
  const stale = rule({ action: "none", templateId: "gone", literal: "x".repeat(10), ackType: "BAD TYPE", ackCode: "zz", delay: "abc" });
  assert.deepEqual(validateRule(stale, TPL), []);
  assert.deepEqual(validateRule(stale, null), []);
  const stored = ruleToPersisted(stale);
  assert.deepEqual(stored.action, { type: "none" });
  assert.equal(stored.delay_ms, 0);
  const back = ruleFromPersisted({ ...stored, delay_ms: 900 });
  assert.equal(back.action, "none");
  assert.equal(back.delay, "0");
  assert.deepEqual(ruleToPersisted(back), stored);
  // A generated-ACK leftover on an ASTM trigger is irrelevant too; real constraints stay.
  assert.deepEqual(validateRule(rule({ action: "none", trigger: "astm_enq" }), TPL), []);
  assert.deepEqual(fields(rule({ action: "none", name: " ", messageType: "A B" })), ["name", "messageType"]);
  assert.deepEqual(fields(rule({ action: "none", useCondition: true, condSegment: "q", condField: "0", condValue: "" })), ["condSegment", "condField", "condValue"]);
  // Switching back re-enables the checks and keeps the draft delay.
  assert.deepEqual(fields({ ...stale, action: "template" }), ["templateId", "delay"]);
  assert.equal(describeAction(stale, TPL), "No auto reply");
  assert.equal(describeDelay(stale), "—");
});

test("validation reports each invalid field", () => {
  assert.deepEqual(fields(rule({ name: " " })), ["name"]);
  assert.deepEqual(fields(rule({ messageType: "" })), ["messageType"]);
  assert.deepEqual(fields(rule({ messageType: "A B" })), ["messageType"]);
  assert.deepEqual(fields(rule({ messageType: "A|B" })), ["messageType"]);
  assert.deepEqual(fields(rule({ useCondition: true, condSegment: "qrd", condField: "0", condValue: "" })), ["condSegment", "condField", "condValue"]);
  assert.deepEqual(fields(rule({ delay: "60001" })), ["delay"]);
  assert.deepEqual(fields(rule({ delay: "-1" })), ["delay"]);
  assert.deepEqual(fields(rule({ delay: "1.5" })), ["delay"]);
  assert.deepEqual(fields(rule({ action: "literal", literal: " " })), ["literal"]);
  assert.deepEqual(fields(rule({ action: "hl7_ack", ackCode: "a" })), ["ackCode"]);
  assert.deepEqual(fields(rule({ action: "hl7_ack", ackType: "AC*" })), ["ackType"]);
  assert.deepEqual(fields(rule({ trigger: "astm_frame", action: "hl7_ack" })), ["action"]);
  assert.deepEqual(fields(rule({ action: "template", templateId: "" })), ["templateId"]);
});

test("template references: missing, unresolved variable, REQ_CONTROL_ID outside HL7, unknown library", () => {
  assert.match(validateRule(rule({ templateId: "nope" }), TPL)[0].message, /no longer exists/);
  assert.match(validateRule(rule({ templateId: "bad" }), TPL)[0].message, /Unresolved variable\(s\): UNSET/);
  assert.deepEqual(fields(rule({ templateId: "adr" })), [], "REQ_CONTROL_ID is filled from the request");
  assert.match(validateRule(rule({ trigger: "astm_frame", templateId: "adr" }), TPL)[0].message, /only available for HL7/);
  assert.deepEqual(fields(rule({ templateId: "nope" }), null), [], "the backend decides when the library is unknown");
  assert.deepEqual(templateProblems(TPL[2], false), []);
});

test("preview resolves automatic variables and shows the request placeholder, nothing else", () => {
  const { text, problems } = previewTemplate(TPL[0], true);
  assert.deepEqual(problems, []);
  assert.match(text, /MSA\|AA\|‹request MSH-10›/);
  assert.match(text, /\|\d{14}\|/);
});

test("validateRules flags duplicate ids per rule", () => {
  const problems = validateRules([rule(), rule()], TPL);
  assert.equal(problems.size, 1);
});

test("moveRule reorders, clamps and never mutates", () => {
  const list = ["a", "b", "c"].map((id) => rule({ id }));
  assert.deepEqual(moveRule(list, "c", -1).map((r) => r.id), ["a", "c", "b"]);
  assert.deepEqual(moveRule(list, "a", 5).map((r) => r.id), ["b", "c", "a"]);
  assert.equal(moveRule(list, "a", -1), list);
  assert.deepEqual(list.map((r) => r.id), ["a", "b", "c"]);
});

test("describe helpers", () => {
  assert.equal(describeCondition(rule()), "Any (otherwise)");
  assert.equal(describeCondition(rule({ useCondition: true, condSegment: "QRD", condField: "8", condOperator: "glob", condValue: "AAZ*" })), "QRD-8 matches AAZ*");
  assert.equal(describeAction(rule({ templateId: "adr" }), TPL), "Template: ADR");
  assert.equal(describeAction(rule({ templateId: "x" }), TPL), "Template: (missing)");
  assert.equal(describeAction(rule({ action: "hl7_ack", ackType: "ACK", ackCode: "AE" }), TPL), "HL7 ACK AE");
});

// ── legacy ──

const storage = (initial) => {
  const data = new Map(initial === undefined ? [] : [["simautomate:config", initial]]);
  return { getItem: (k) => data.get(k) ?? null, setItem: (k, v) => data.set(k, v), raw: () => data.get("simautomate:config") };
};

test("legacy settings become explicit rules and never enable replies", () => {
  const s = storage(JSON.stringify({ "autoresponse-enabled": true, astm_ack: "<ACK>", hl7_type: "ACK^O21", hl7_code: "AE", other: "keep" }));
  const legacy = readLegacySettings(s);
  assert.equal(legacy.wasEnabled, true);
  assert.deepEqual(legacy.rules.map((r) => [r.trigger, r.action]), [["astm_enq", "literal"], ["astm_frame", "literal"], ["hl7", "hl7_ack"]]);
  assert.equal(legacy.rules[0].literal, "<ACK>");
  assert.equal(legacy.rules[2].ackType, "ACK^O21");
  assert.equal(legacy.rules[2].ackCode, "AE");
  for (const r of legacy.rules) assert.deepEqual(validateRule(r, TPL), []);
  assert.equal(s.raw().includes("astm_ack"), true, "reading removes nothing");
  removeLegacySettings(s);
  assert.deepEqual(JSON.parse(s.raw()), { other: "keep" });
});

test("no legacy keys, no storage entry, and corrupt data are distinguished", () => {
  assert.equal(readLegacySettings(storage()), null);
  assert.equal(readLegacySettings(storage(JSON.stringify({ other: 1 }))), null);
  assert.match(readLegacySettings(storage("{broken")).error, /corrupted/);
  assert.match(readLegacySettings(storage("[1]")).error, /corrupted/);
});
