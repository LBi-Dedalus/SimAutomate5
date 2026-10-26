import test from "node:test";
import assert from "node:assert/strict";
import {
  applyAstmUnit,
  closeAstm,
  createAstmState,
  expireAstm,
  partitionFrames,
  replyCounts,
  ASTM_IDLE_TIMEOUT_MS,
} from "../src/session/astm-group.js";
import { createSessionStore } from "../src/session/sessions-core.js";
import { describeMessage } from "../src/session/inspector-core.js";

const T0 = Date.parse("2024-01-01T00:00:00Z");
const stamp = (offsetMs = 0) => new Date(T0 + offsetMs).toISOString();

const unit = (msg_type, astm, at = 0) => ({
  session_id: "s1",
  attempt: 1,
  msg_type,
  content: astm.kind === "frame" ? `<STX>${astm.number}${astm.text}<${astm.terminator}>` : `<${astm.kind.toUpperCase()}>`,
  raw: [1, 2, 3],
  timestamp: stamp(at),
  astm,
});
const frame = (number, text, terminator = "ETX", checksum_ok = true) => ({
  kind: "frame",
  number,
  text,
  terminator,
  checksum: "00",
  computed_checksum: checksum_ok ? "00" : "7D",
  checksum_ok,
});
const enq = { kind: "enq" };
const ack = { kind: "ack" };
const nak = { kind: "nak" };
const eot = { kind: "eot" };

function harness() {
  const state = createAstmState();
  const records = [];
  const create = (fields) => {
    const record = { id: records.length + 1, ...fields };
    records.push(record);
    return record;
  };
  return { state, records, apply: (payload) => applyAstmUnit(state, payload, create) };
}

test("a clean transfer becomes one record with ACK hints and EOT", () => {
  const h = harness();
  h.apply(unit("received", enq, 0));
  h.apply(unit("sent", ack, 10));
  h.apply(unit("received", frame(1, "H|\\^&\rP|1\r", "ETB"), 20));
  h.apply(unit("sent", ack, 30));
  h.apply(unit("received", frame(2, "L|1|N\r"), 40));
  h.apply(unit("sent", ack, 50));
  h.apply(unit("received", eot, 60));

  assert.equal(h.records.length, 1);
  const [record] = h.records;
  assert.equal(record.msg_type, "received");
  assert.equal(record.transfer.status, "complete");
  assert.deepEqual(
    record.transfer.entries.map((e) => [e.kind, e.reply?.kind ?? null]),
    [["enq", "ack"], ["frame", "ack"], ["frame", "ack"], ["eot", null]],
  );
  assert.equal(h.state.open.received, null);
  assert.deepEqual(replyCounts(record.transfer), { ack: 2, nak: 0, none: 0, frames: 2 });
  const { merged, apart } = partitionFrames(record.transfer);
  assert.equal(merged.length, 2);
  assert.equal(apart.length, 0);
  assert.equal(record.content, "<ENQ>\n<STX>1H|\\^&\rP|1\r<ETB>\n<STX>2L|1|N\r<ETX>\n<EOT>");
  assert.equal(record.raw.length, 12);
});

test("a NAK keeps the frame apart and its retransmission is a new row", () => {
  const h = harness();
  h.apply(unit("sent", enq));
  h.apply(unit("received", ack));
  h.apply(unit("sent", frame(1, "H|\\^&\r", "ETB")));
  h.apply(unit("received", nak));
  h.apply(unit("sent", frame(1, "H|\\^&\r", "ETB")));
  h.apply(unit("received", ack));
  h.apply(unit("sent", eot));

  assert.equal(h.records.length, 1);
  const { transfer } = h.records[0];
  assert.equal(transfer.status, "complete");
  const { merged, apart } = partitionFrames(transfer);
  assert.equal(merged.length, 1);
  assert.equal(apart.length, 1);
  assert.equal(apart[0].reply.kind, "nak");
  assert.deepEqual(replyCounts(transfer), { ack: 1, nak: 1, none: 0, frames: 2 });
});

test("a frame without answer stays apart once followed by another one", () => {
  const h = harness();
  h.apply(unit("sent", frame(1, "H|\\^&\r", "ETB")));
  h.apply(unit("sent", frame(2, "P|1\r", "ETB")));
  h.apply(unit("received", ack));

  const { transfer } = h.records[0];
  assert.equal(transfer.implicit, true);
  const { merged, apart } = partitionFrames(transfer);
  assert.equal(merged.length, 1, "only the answered one");
  assert.equal(apart.length, 1);
  assert.equal(apart[0].frame.number, 1);
});

test("the last frame of an open transfer is merged while it waits for its answer", () => {
  const h = harness();
  h.apply(unit("sent", frame(1, "H|\\^&\r", "ETB")));
  const { merged, apart } = partitionFrames(h.records[0].transfer);
  assert.equal(merged.length, 1);
  assert.equal(apart.length, 0);
});

test("a new ENQ closes the previous transfer as incomplete", () => {
  const h = harness();
  h.apply(unit("received", enq));
  h.apply(unit("received", frame(1, "H|\\^&\r", "ETB")));
  const { updated } = h.apply(unit("received", enq));

  assert.equal(h.records.length, 2);
  assert.equal(h.records[0].transfer.status, "incomplete");
  assert.equal(h.records[1].transfer.status, "open");
  assert.ok(updated.includes(h.records[0]));
});

test("a frame after an ETX frame starts a new implicit message", () => {
  const h = harness();
  h.apply(unit("received", frame(1, "H|\\^&\rL|1\r")));
  h.apply(unit("sent", ack));
  h.apply(unit("received", frame(1, "H|\\^&\rL|1\r")));

  assert.equal(h.records.length, 2);
  assert.equal(h.records[0].transfer.status, "incomplete");
  assert.equal(h.records[1].transfer.implicit, true);
});

test("a stale transfer is closed lazily before the next event", () => {
  const h = harness();
  h.apply(unit("received", enq, 0));
  h.apply(unit("sent", enq, 100_000));
  assert.equal(h.records[0].transfer.status, "incomplete");
  h.apply(unit("sent", frame(1, "H|\\^&\r", "ETB"), 200_000));
  assert.equal(h.records.length, 3, "the stale transfer was closed before the frame");
  assert.equal(h.records[1].transfer.status, "incomplete");
});

test("expireAstm closes only the open transfer of its direction", () => {
  const h = harness();
  h.apply(unit("received", enq));
  h.apply(unit("sent", enq));
  assert.deepEqual(expireAstm(h.state, "received"), [h.records[0]]);
  assert.equal(h.records[0].transfer.status, "incomplete");
  assert.equal(h.records[1].transfer.status, "open");
  assert.deepEqual(expireAstm(h.state, "received"), []);
});

test("an ACK or NAK nobody is waiting for is a plain record", () => {
  const h = harness();
  const { created } = h.apply(unit("received", ack));
  assert.equal(h.records.length, 1);
  assert.equal(created.transfer, undefined);
  assert.equal(describeMessage(created).kind, "astm");
});

test("an EOT from the other side aborts the transfer instead of answering a frame", () => {
  const h = harness();
  h.apply(unit("sent", frame(1, "H|\\^&\r", "ETB")));
  h.apply(unit("received", eot));

  assert.equal(h.records.length, 1);
  const { transfer } = h.records[0];
  assert.equal(transfer.status, "incomplete");
  assert.equal(transfer.entries[0].reply.kind, "eot");
});

test("a bad checksum flags the transfer but stays merged", () => {
  const h = harness();
  h.apply(unit("received", frame(1, "H|\\^&\r", "ETB", false)));
  h.apply(unit("sent", ack));
  const { transfer } = h.records[0];
  assert.equal(transfer.hasErrors, true);
  assert.equal(partitionFrames(transfer).merged.length, 1);
});

test("both directions can be open at the same time", () => {
  const h = harness();
  h.apply(unit("received", enq));
  h.apply(unit("sent", ack));
  h.apply(unit("sent", enq));
  assert.equal(h.records.length, 2);
  assert.notEqual(h.state.open.received, null);
  assert.notEqual(h.state.open.sent, null);
  assert.equal(closeAstm(h.state).length, 2);
});

const makeStore = () => {
  let n = 0;
  return createSessionStore({ newId: () => `s${++n}`, now: () => "T", onListenerError: (e) => { throw e; } });
};

test("the store groups ASTM units, emits updates and keeps its counters", () => {
  const store = makeStore();
  const session = store.create({ mode: "client", label: "A", req: {} });
  store.beginAttempt(session.id);
  const events = [];
  store.subscribe((event) => events.push(event.type));

  const feed = (payload) => store.applyMessage({ ...payload, session_id: session.id, attempt: session.attempt });
  feed(unit("received", enq));
  feed(unit("sent", ack));
  feed(unit("received", frame(1, "H|\\^&\rL|1|N\r")));
  feed(unit("sent", ack));
  feed(unit("received", eot));

  assert.equal(session.records.length, 1);
  assert.equal(session.received, 1);
  assert.equal(events.filter((type) => type === "message").length, 1);
  assert.ok(events.filter((type) => type === "record-updated").length >= 4);
  assert.equal(session.records[0].transfer.status, "complete");
});

test("an open transfer is closed when the connection ends and dropped on clear", () => {
  const store = makeStore();
  const session = store.create({ mode: "client", label: "A", req: {} });
  store.beginAttempt(session.id);
  store.applyMessage({ ...unit("received", enq), session_id: session.id, attempt: session.attempt });
  store.applyStatus({ session_id: session.id, attempt: session.attempt, status: "disconnected" });
  assert.equal(session.records[0].transfer.status, "incomplete");

  store.applyMessage({ ...unit("received", enq), session_id: session.id, attempt: session.attempt });
  store.clearMessages(session.id);
  assert.equal(session.astm.open.received, null);
  assert.equal(session.sent, 0);
});

/** A store whose timers are fake: `fire(timer)` runs a pending one. */
function timerStore() {
  const timers = new Set();
  const created = [];
  let n = 0;
  const store = createSessionStore({
    newId: () => `s${++n}`,
    now: () => "T",
    onListenerError: (e) => {
      throw e;
    },
    setTimer: (fn, ms) => {
      const timer = { fn, ms };
      timers.add(timer);
      created.push(timer);
      return timer;
    },
    clearTimer: (timer) => timers.delete(timer),
  });
  const fire = (timer) => {
    assert.ok(timers.delete(timer), "the timer was still pending");
    timer.fn();
  };
  const open = () => {
    const session = store.create({ mode: "client", label: "A", req: {} });
    store.beginAttempt(session.id);
    const feed = (payload) =>
      store.applyMessage({ ...payload, session_id: session.id, attempt: session.attempt });
    return { session, feed };
  };
  return { store, timers, created, fire, open };
}

test("one idle timer per open transfer is armed, re-armed on events and fires as incomplete", () => {
  const { store, timers, created, fire, open } = timerStore();
  const { session, feed } = open();
  const updates = [];
  store.subscribe((event) => event.type === "record-updated" && updates.push(event.record));

  feed(unit("received", enq, 0));
  assert.equal(timers.size, 1);
  assert.equal(created[0].ms, ASTM_IDLE_TIMEOUT_MS);

  feed(unit("sent", ack, 10)); // touches the received transfer: re-armed
  assert.equal(timers.size, 1);
  assert.equal(created.length, 2);
  assert.ok(!timers.has(created[0]));

  feed(unit("sent", enq, 20)); // the other direction has its own timer
  assert.equal(timers.size, 2);

  const record = session.records[0];
  updates.length = 0;
  fire(created[1]);
  assert.equal(record.transfer.status, "incomplete");
  assert.deepEqual(updates, [record], "the view is told without any new event");
  assert.equal(timers.size, 1);
  assert.equal(session.records[1].transfer.status, "open");
});

test("no idle timer survives a closed transfer, a clear, a new attempt or a removed session", () => {
  const { store, timers, open } = timerStore();

  let { session, feed } = open();
  feed(unit("received", enq));
  feed(unit("received", eot));
  assert.equal(timers.size, 0, "EOT closed the transfer");

  feed(unit("received", enq));
  store.clearMessages(session.id);
  assert.equal(timers.size, 0, "clear");

  feed(unit("received", enq));
  store.beginAttempt(session.id);
  assert.equal(timers.size, 0, "reconnect");

  ({ session, feed } = open());
  feed(unit("received", enq));
  feed(unit("sent", enq));
  assert.equal(timers.size, 2);
  store.remove(session.id);
  assert.equal(timers.size, 0, "session removed");
});

test("a new ENQ replaces the timer of the transfer it closes and eviction clears it", () => {
  const { store, timers, open } = timerStore();
  const { session, feed } = open();
  feed(unit("received", enq));
  feed(unit("received", enq));
  assert.equal(timers.size, 1);
  assert.equal(session.records[0].transfer.status, "incomplete");

  for (let i = 0; i < 2000; i += 1) store.addLocal(session.id, "system", "x");
  assert.equal(session.astm.open.received, null, "the open record was evicted");
  assert.equal(timers.size, 0);
});

test("the default timers are real, unref'ed ones that never keep the process alive", () => {
  const store = createSessionStore({ newId: () => "s1", now: () => "T" });
  const session = store.create({ mode: "client", label: "A", req: {} });
  store.beginAttempt(session.id);
  store.applyMessage({ ...unit("received", enq), session_id: "s1", attempt: session.attempt });
  store.remove("s1"); // clears the timer; the test process must exit by itself
});
