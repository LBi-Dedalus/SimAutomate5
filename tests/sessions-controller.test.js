import test from "node:test";
import assert from "node:assert/strict";
import { createSessionController } from "../src/sessions-controller.js";
import { createSessionStore } from "../src/sessions-core.js";
import { buildForTarget } from "../src/autobuild-core.js";

const memoryStorage = () => {
  const data = {};
  return { getItem: (k) => data[k] ?? null, setItem: (k, v) => { data[k] = v; } };
};

function setup(handler = async () => {}) {
  const calls = [];
  const logs = [];
  let n = 0;
  const store = createSessionStore({ newId: () => `s${++n}`, now: () => "T", onListenerError: (e) => { throw e; } });
  const controller = createSessionController({
    invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      return handler(cmd, args, controller);
    },
    storage: memoryStorage(),
    log: { error: (m) => logs.push(m) },
    store,
  });
  return { controller, store, calls, logs };
}

const req = { type: "ClientConnectRequest", ip: "h", port: 1 };
const status = (s, st, attempt = s.attempt) => ({ session_id: s.id, attempt, status: st });
const incoming = (s, content, attempt = s.attempt) => ({ session_id: s.id, attempt, msg_type: "received", content, timestamp: "T" });

test("events emitted before connect resolves are kept (session registered first)", async () => {
  const { controller, store } = setup(async (cmd, args, c) => {
    if (cmd === "connect_socket") {
      const s = store.get(args.sessionId);
      c.handleStatus(status(s, "connected"));
      c.handleMessage(incoming(s, "early"));
    }
  });
  const { id, done } = controller.start({ mode: "client", label: "A", req });
  await done;
  const s = store.get(id);
  assert.equal(s.status, "connected");
  assert.equal(s.records.length, 1);
  assert.equal(s.registered, true);
});

test("background session keeps receiving; selected status is per session", async () => {
  const { controller, store } = setup();
  const a = controller.start({ mode: "client", label: "A", req });
  const b = controller.start({ mode: "client", label: "B", req });
  await Promise.all([a.done, b.done]);
  store.select(a.id);
  controller.handleStatus(status(store.get(a.id), "connected"));
  controller.handleStatus(status(store.get(b.id), "error"));
  controller.handleMessage(incoming(store.get(b.id), "bg"));
  assert.equal(store.active().status, "connected");
  assert.equal(store.get(b.id).status, "error");
  assert.equal(store.get(b.id).records.length, 1);
  assert.equal(store.get(a.id).records.length, 0);
});

test("a reconnect ignores late events of the previous attempt", async () => {
  const { controller, store, calls } = setup();
  const { id, done } = controller.start({ mode: "client", label: "A", req });
  await done;
  const s = store.get(id);
  controller.handleStatus(status(s, "error"));
  const old = s.attempt;
  await controller.reconnect(id);
  assert.equal(s.attempt, old + 1);
  assert.equal(calls.filter((c) => c.cmd === "connect_socket").at(-1).args.attempt, old + 1);
  controller.handleStatus(status(s, "error", old));
  controller.handleMessage(incoming(s, "late", old));
  assert.equal(s.status, "connecting");
  assert.equal(s.records.length, 0);
});

test("a failed connect marks only that attempt and shows an error in that session", async () => {
  const { controller, store } = setup(async () => {
    throw "port in use";
  });
  const { id, done } = controller.start({ mode: "server", label: "A", req: { type: "ServerStartRequest", port: 1 } });
  const result = await done;
  assert.equal(result.ok, false);
  const s = store.get(id);
  assert.equal(s.status, "error");
  assert.match(s.records[0].content, /port in use/);
  assert.equal(s.registered, false);
});

test("close removes the session; late events for it create nothing", async () => {
  const { controller, store, calls } = setup();
  const { id, done } = controller.start({ mode: "client", label: "A", req });
  await done;
  const s = store.get(id);
  await controller.close(id);
  assert.equal(store.get(id), null);
  assert.deepEqual(calls.at(-1), { cmd: "close_session", args: { sessionId: id } });
  assert.equal(controller.handleStatus(status(s, "connected")).accepted, false);
  assert.equal(controller.handleMessage(incoming(s, "late")), null);
  assert.equal(store.list().length, 0);
});

test("a failed close keeps the session and reports the error", async () => {
  const { controller, store } = setup(async (cmd) => {
    if (cmd === "close_session") throw "boom";
  });
  const { id, done } = controller.start({ mode: "client", label: "A", req });
  await done;
  const result = await controller.close(id);
  assert.equal(result.ok, false);
  const s = store.get(id);
  assert.ok(s);
  assert.equal(s.op, null);
  assert.match(s.records.at(-1).content, /boom/);
});

test("a never-registered session is closed locally without a backend call", async () => {
  const { controller, store, calls } = setup(async () => {
    throw "bad";
  });
  const { id, done } = controller.start({ mode: "client", label: "A", req });
  await done;
  await controller.close(id);
  assert.equal(store.get(id), null);
  assert.equal(calls.some((c) => c.cmd === "close_session"), false);
});

test("sends go to the captured session, even after a switch, and only when connected", async () => {
  const { controller, store, calls } = setup();
  const a = controller.start({ mode: "client", label: "A", req });
  const b = controller.start({ mode: "client", label: "B", req });
  await Promise.all([a.done, b.done]);
  controller.handleStatus(status(store.get(a.id), "connected"));
  store.select(a.id);
  const target = controller.sendTarget();
  assert.equal(target.id, a.id);
  store.select(b.id);
  assert.equal(controller.sendTarget(), null, "B is still connecting: nothing to send to");
  const result = await controller.send(target, "hello");
  assert.equal(result.ok, true);
  const sent = calls.filter((c) => c.cmd === "send_message");
  assert.equal(sent.length, 1);
  assert.equal(sent[0].args.sessionId, a.id);
  assert.equal(sent[0].args.attempt, target.attempt);
});

test("a send to a closed or disconnected target is refused visibly, never redirected", async () => {
  const { controller, store, calls } = setup();
  const a = controller.start({ mode: "client", label: "A", req });
  const b = controller.start({ mode: "client", label: "B", req });
  await Promise.all([a.done, b.done]);
  controller.handleStatus(status(store.get(a.id), "connected"));
  controller.handleStatus(status(store.get(b.id), "connected"));
  const target = controller.sendTarget(a.id);
  controller.handleStatus(status(store.get(a.id), "disconnected"));
  assert.equal((await controller.send(target, "x")).ok, false);
  assert.match(store.get(a.id).records.at(-1).content, /not connected/);
  await controller.close(a.id);
  assert.equal((await controller.send(target, "x")).ok, false);
  assert.equal(calls.filter((c) => c.cmd === "send_message").length, 0);
  assert.equal(store.get(b.id).records.length, 0, "B untouched");
});

test("a backend send failure is shown in the target session only", async () => {
  const { controller, store } = setup(async (cmd) => {
    if (cmd === "send_message") throw "rejected";
  });
  const a = controller.start({ mode: "client", label: "A", req });
  const b = controller.start({ mode: "client", label: "B", req });
  await Promise.all([a.done, b.done]);
  controller.handleStatus(status(store.get(a.id), "connected"));
  const target = controller.sendTarget(a.id);
  store.select(b.id);
  assert.equal((await controller.send(target, "x")).ok, false);
  assert.match(store.get(a.id).records.at(-1).content, /rejected/);
  assert.equal(store.get(b.id).records.length, 0);
});

test("disconnect stops only the chosen session and it can reconnect", async () => {
  const { controller, store, calls } = setup();
  const a = controller.start({ mode: "client", label: "A", req });
  const b = controller.start({ mode: "client", label: "B", req });
  await Promise.all([a.done, b.done]);
  controller.handleStatus(status(store.get(a.id), "connected"));
  controller.handleStatus(status(store.get(b.id), "connected"));
  await controller.disconnect(a.id);
  assert.equal(store.get(a.id).status, "disconnected");
  assert.equal(store.get(b.id).status, "connected");
  assert.equal(calls.filter((c) => c.cmd === "disconnect_socket").length, 1);
  assert.equal((await controller.reconnect(a.id)).ok, true);
  assert.equal(store.get(a.id).attempt, 2);
});

test("a send mid-disconnect is refused", async () => {
  let release;
  const gate = new Promise((r) => (release = r));
  const { controller, store } = setup(async (cmd) => {
    if (cmd === "disconnect_socket") await gate;
  });
  const a = controller.start({ mode: "client", label: "A", req });
  await a.done;
  controller.handleStatus(status(store.get(a.id), "connected"));
  const pending = controller.disconnect(a.id);
  assert.equal(controller.sendTarget(a.id), null);
  release();
  await pending;
});

test("autobuild delivers to the session captured before the await", async () => {
  let active = "A";
  const drafts = {};
  let release;
  const gate = new Promise((r) => (release = r));
  const deps = {
    invoke: async () => {
      await gate;
      return { output: "BUILT" };
    },
    activeId: () => active,
    hasSession: (id) => id in { A: 1, B: 1 },
    setDraft: (id, text) => {
      drafts[id] = text;
      return true;
    },
  };
  const pending = buildForTarget(deps, "in", false, { deliver: true });
  active = "B";
  release();
  const result = await pending;
  assert.deepEqual(drafts, { A: "BUILT" });
  assert.equal(result.delivered, true);
});

test("autobuild reports a closed or missing target instead of dropping output", async () => {
  const base = { invoke: async () => ({ output: "O" }), setDraft: () => true };
  const none = await buildForTarget({ ...base, activeId: () => null, hasSession: () => false }, "i", false, { deliver: true });
  assert.equal(none.delivered, false);
  assert.equal(none.output, "O");
  assert.match(none.error, /No session/);
  const closed = await buildForTarget({ ...base, activeId: () => "A", hasSession: () => false }, "i", false, { deliver: true });
  assert.match(closed.error, /closed/);
  const preview = await buildForTarget({ ...base, activeId: () => null, hasSession: () => false }, "i", false, { deliver: false });
  assert.equal(preview.error, null);
});
