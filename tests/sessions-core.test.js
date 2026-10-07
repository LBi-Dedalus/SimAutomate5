import test from "node:test";
import assert from "node:assert/strict";
import { createSessionStore, MAX_MESSAGES, takeLegacyDraft } from "../src/sessions-core.js";

const make = () => {
  let n = 0;
  return createSessionStore({ newId: () => `s${++n}`, now: () => "T", onListenerError: (e) => { throw e; } });
};
const open = (store, label) => {
  const s = store.create({ mode: "client", label, req: {} });
  store.beginAttempt(s.id);
  return s;
};
const msg = (s, type, content) => ({
  session_id: s.id,
  attempt: s.attempt,
  msg_type: type,
  content,
  timestamp: "T",
});

test("history, counters, draft and selection are isolated per session", () => {
  const store = make();
  const a = open(store, "A");
  const b = open(store, "B");
  store.select(a.id);
  store.applyMessage(msg(a, "received", "x"));
  store.applyMessage(msg(a, "sent", "y"));
  store.applyMessage(msg(b, "received", "z"));
  store.setDraft(a.id, "draft A");
  store.selectRecord(a.id, 1);

  assert.equal(a.records.length, 2);
  assert.equal(b.records.length, 1);
  assert.deepEqual([a.sent, a.received, b.sent, b.received], [1, 1, 0, 1]);
  assert.equal(b.draft, "");
  assert.equal(b.selectedRecordId, null);
  assert.equal(a.selectedRecordId, 1);

  store.clearMessages(a.id);
  assert.equal(a.records.length, 0);
  assert.equal(a.draft, "draft A", "clearing history keeps the draft");
  assert.equal(b.records.length, 1, "clearing A keeps B");
});

test("each session keeps its own 2000 message cap", () => {
  const store = make();
  const a = open(store, "A");
  const b = open(store, "B");
  for (let i = 0; i < MAX_MESSAGES + 5; i++) store.applyMessage(msg(a, "received", `m${i}`));
  store.applyMessage(msg(b, "received", "only"));
  assert.equal(a.records.length, MAX_MESSAGES);
  assert.equal(a.records[0].content, "m5");
  assert.equal(a.received, MAX_MESSAGES);
  assert.equal(b.records.length, 1);
});

test("unknown ids and stale attempts are dropped and never create a session", () => {
  const store = make();
  const a = open(store, "A");
  assert.deepEqual(store.applyStatus({ session_id: "ghost", attempt: 1, status: "connected" }), { accepted: false, reason: "unknown" });
  assert.equal(store.applyMessage({ session_id: "ghost", attempt: 1, msg_type: "received", content: "x" }), null);
  assert.equal(store.list().length, 1);

  const old = a.attempt;
  store.beginAttempt(a.id);
  assert.equal(store.applyStatus({ session_id: a.id, attempt: old, status: "error" }).reason, "stale");
  assert.equal(a.status, "connecting");
  assert.equal(store.applyMessage({ session_id: a.id, attempt: old, msg_type: "received", content: "late" }), null);
  assert.equal(store.applyStatus({ session_id: a.id, attempt: a.attempt, status: "connected" }).accepted, true);
});

test("background messages count as unread until selected; removal selects a neighbour", () => {
  const store = make();
  const a = open(store, "A");
  const b = open(store, "B");
  store.select(a.id);
  store.applyMessage(msg(b, "received", "hi"));
  assert.equal(b.unread, 1);
  store.select(b.id);
  assert.equal(b.unread, 0);
  store.remove(b.id);
  assert.equal(store.activeId, a.id);
  store.remove(a.id);
  assert.equal(store.activeId, null);
  assert.equal(store.active(), null);
});

test("legacy composer text becomes a one-time draft", () => {
  const data = { "simautomate:config": JSON.stringify({ message: "old", host: "h" }) };
  const storage = { getItem: (k) => data[k] ?? null, setItem: (k, v) => { data[k] = v; } };
  const first = takeLegacyDraft(storage);
  assert.equal(first.draft, "old");
  assert.equal(takeLegacyDraft(storage).draft, "", "consumed once");
  assert.equal(JSON.parse(Object.values(data)[0]).host, "h", "other fields are preserved");
});
