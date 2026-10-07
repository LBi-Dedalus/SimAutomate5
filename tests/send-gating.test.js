// Integration: the composer Send, the control-character buttons and the template Send
// (real modules: sessions, messages, special-chars, templates) all follow ONE eligibility
// rule, sessions.sendTarget(), including while a disconnect/close is pending.
import test from "node:test";
import assert from "node:assert/strict";

class FakeNode {
  constructor(tag = "div", id = "") {
    this.tag = tag;
    this.id = id;
    this.children = [];
    this.listeners = {};
    this.classes = new Set();
    this.dataset = {};
    this.style = {};
    this.value = "";
    this.disabled = false;
    this.textContent = "";
    this.title = "";
    const self = this;
    this.classList = {
      add: (c) => self.classes.add(c),
      remove: (c) => self.classes.delete(c),
      contains: (c) => self.classes.has(c),
      toggle(c, force) {
        const on = force === undefined ? !self.classes.has(c) : force;
        if (on) self.classes.add(c);
        else self.classes.delete(c);
      },
    };
  }
  set className(v) {
    this.classes = new Set(String(v).split(/\s+/).filter(Boolean));
  }
  get className() {
    return [...this.classes].join(" ");
  }
  get hidden() {
    return this.classes.has("hidden");
  }
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  removeEventListener() {}
  fire(type) {
    for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {} });
  }
  appendChild(c) {
    this.children.push(c);
    return c;
  }
  append(...c) {
    this.children.push(...c);
  }
  remove() {}
  replaceChildren() {
    this.children = [];
  }
  setAttribute() {}
  focus() {}
  select() {}
  scrollIntoView() {}
  toggleAttribute() {}
  contains() {
    return false;
  }
  find(pred, out = []) {
    for (const c of this.children) {
      if (pred(c)) out.push(c);
      c.find?.(pred, out);
    }
    return out;
  }
}

const nodes = new Map();
const docListeners = {};
const winListeners = {};
const getNode = (id) => {
  if (!nodes.has(id)) nodes.set(id, new FakeNode("div", id));
  return nodes.get(id);
};

// Composer form: `message` field and one submit button.
const composerSend = new FakeNode("button", "composer-send");
const composerForm = getNode("message-form");
composerForm.message = new FakeNode("textarea", "message");
composerForm.querySelector = (sel) => (sel === 'button[type="submit"]' ? composerSend : null);
// Control characters.
const ctrlButtons = ["STX", "ETX"].map((t) => {
  const b = new FakeNode("button", `ctrl-${t}`);
  b.dataset.token = t;
  return b;
});
getNode("special-chars").querySelectorAll = () => ctrlButtons;

globalThis.document = {
  getElementById: getNode,
  createElement: (tag) => new FakeNode(tag),
  createTextNode: (t) => ({ textContent: t }),
  querySelectorAll(sel) {
    if (sel === "#tpl-vars input") return getNode("tpl-vars").find((n) => n.tag === "input");
    return [];
  },
  addEventListener: (type, fn) => (docListeners[type] ??= []).push(fn),
  dispatchEvent: () => {},
};
const ipc = { calls: [], pending: new Map() };
/** Holds a command until the test settles it. */
const hold = (cmd) => {
  let resolve, reject;
  const promise = new Promise((res, rej) => ((resolve = res), (reject = rej)));
  ipc.pending.set(cmd, { promise, resolve, reject });
};
globalThis.window = {
  addEventListener: (type, fn) => (winListeners[type] ??= []).push(fn),
  __TAURI__: {
    core: {
      invoke: async (cmd, args) => {
        ipc.calls.push({ cmd, args });
        if (cmd === "load_templates") return { templates: [{ id: "a", name: "Alpha", description: "", payload: "ALPHA", variables: [] }], seeded: false };
        const held = ipc.pending.get(cmd);
        if (held) return held.promise;
        return undefined;
      },
    },
    event: { listen: async () => {} },
  },
  connection_status: { get: () => "disconnected", set() {}, subscribe() {} },
};

const { sessions, store } = await import("../src/sessions.js");
await import("../src/messages.js");
await import("../src/special-chars.js");
await import("../src/templates.js");

for (const fn of [...(winListeners.DOMContentLoaded ?? []), ...(docListeners.DOMContentLoaded ?? [])]) await fn();

const tick = () => new Promise((r) => setTimeout(r, 0));
const surfaces = () => ({
  composer: composerSend.disabled,
  controls: ctrlButtons.map((b) => b.disabled),
  template: getNode("tpl-send").disabled,
});
const assertAll = (disabled, why) => {
  const s = surfaces();
  assert.equal(s.composer, disabled, `composer Send ${why}`);
  assert.deepEqual(s.controls, [disabled, disabled], `control buttons ${why}`);
  assert.equal(s.template, disabled, `template Send ${why}`);
};
const connect = (session) => sessions.handleStatus({ session_id: session.id, attempt: session.attempt, status: "connected" });
const startConnected = async (port) => {
  const { id, done } = sessions.start({ mode: "client", label: `h:${port}`, req: { type: "ClientConnectRequest", ip: "h", port } });
  await done;
  connect(store.get(id));
  return id;
};

test("no session: every send surface is disabled", () => {
  assertAll(true, "without session");
});

for (const kind of ["disconnect", "close"]) {
  test(`pending ${kind}: all send surfaces disable at once and stay disabled until it ends`, async () => {
    const id = await startConnected(kind === "close" ? 2 : 1);
    store.select(id);
    assertAll(false, "when connected");

    const cmd = kind === "close" ? "close_session" : "disconnect_socket";
    hold(cmd);
    const run = kind === "close" ? sessions.close(id) : sessions.disconnect(id);
    assertAll(true, `immediately while ${kind} is pending`);
    assert.equal(store.get(id).status, "connected", "the displayed status is not faked");
    await tick();
    assertAll(true, `while ${kind} is in flight`);

    if (kind === "disconnect") {
      ipc.pending.get(cmd).reject(new Error("boom")); // failure: op restored, still connected
      await run;
      ipc.pending.delete(cmd);
      assertAll(false, "after a failed disconnect (still connected, op ended)");
      // A real disconnect now.
      hold(cmd);
      const second = sessions.disconnect(id);
      assertAll(true, "second disconnect pending");
      ipc.pending.get(cmd).resolve();
      await second;
      ipc.pending.delete(cmd);
      assert.equal(store.get(id).status, "disconnected");
      assertAll(true, "disconnected");
    } else {
      ipc.pending.get(cmd).resolve();
      await run;
      ipc.pending.delete(cmd);
      assert.equal(store.get(id), null);
      assertAll(true, "after the session was closed");
    }
  });
}

test("failed close restores eligibility; switching to an eligible background session enables sending", async () => {
  const a = await startConnected(10);
  const b = await startConnected(11);
  store.select(a);
  hold("close_session");
  const closing = sessions.close(a);
  assertAll(true, "selected session closing");
  store.select(b);
  assertAll(false, "selected another connected session");
  store.select(a);
  assertAll(true, "back on the closing session");
  ipc.pending.get("close_session").reject(new Error("nope"));
  await closing;
  ipc.pending.delete("close_session");
  assert.equal(store.get(a).op, null);
  assertAll(false, "close failed, session still connected");

  // Listening sessions never send.
  const l = sessions.start({ mode: "server", label: ":5000", req: { type: "ServerStartRequest", port: 5000 } });
  await l.done;
  sessions.handleStatus({ session_id: l.id, attempt: store.get(l.id).attempt, status: "listening" });
  store.select(l.id);
  assertAll(true, "listening session");
});

const emitConnected = (id) => sessions.handleStatus({ session_id: id, attempt: store.get(id).attempt, status: "connected" });
const countCalls = (cmd) => ipc.calls.filter((c) => c.cmd === cmd).length;
const startPending = (port) => {
  hold("connect_socket");
  const s = sessions.start({ mode: "client", label: `p:${port}`, req: { type: "ClientConnectRequest", ip: "h", port } });
  store.select(s.id);
  return s;
};

test("disconnect requested while the initial connect is pending stays gated through an early connected event", async () => {
  const { id, done } = startPending(20);
  await tick();
  assert.equal(store.get(id).registered, false);
  hold("disconnect_socket");
  const run = sessions.disconnect(id);
  emitConnected(id); // backend event before connect_socket resolved
  assert.equal(store.get(id).status, "connected");
  assertAll(true, "early connected after disconnect request");
  assert.equal(sessions.sendTarget(id), null);
  const stale = { id, attempt: store.get(id).attempt, label: "x" };
  const sends = countCalls("send_message");
  assert.equal((await sessions.send(stale, "hi")).ok, false);
  assert.equal(countCalls("send_message"), sends, "no send_message invoked");

  ipc.pending.get("connect_socket").resolve();
  ipc.pending.delete("connect_socket");
  await done;
  await tick();
  assert.equal(store.get(id).op, "disconnecting");
  assertAll(true, "queued disconnect pending after connect resolved");
  ipc.pending.get("disconnect_socket").resolve();
  ipc.pending.delete("disconnect_socket");
  assert.equal((await run).ok, true);
  assert.equal(store.get(id).op, null);
  assert.equal(store.get(id).status, "disconnected");
  assertAll(true, "terminal after disconnect");
});

test("disconnect right after a queued reconnect stays gated through an early connected event", async () => {
  const id = await startConnected(21);
  store.select(id);
  await sessions.disconnect(id);
  assert.equal(store.get(id).status, "disconnected");
  hold("connect_socket");
  hold("disconnect_socket");
  const re = sessions.reconnect(id);
  const run = sessions.disconnect(id); // same tick, before the reconnect starts
  assert.equal(store.get(id).op, "disconnecting");
  await tick(); // reconnect invoked, connect_socket still pending
  emitConnected(id);
  assertAll(true, "early connected during reconnect with disconnect requested");
  assert.equal(sessions.sendTarget(id), null);
  const stale = { id, attempt: store.get(id).attempt, label: "x" };
  const sends = countCalls("send_message");
  assert.equal((await sessions.send(stale, "hi")).ok, false);
  assert.equal(countCalls("send_message"), sends);

  ipc.pending.get("connect_socket").resolve();
  ipc.pending.delete("connect_socket");
  await re;
  await tick();
  assertAll(true, "disconnect pending after reconnect resolved");
  ipc.pending.get("disconnect_socket").resolve();
  ipc.pending.delete("disconnect_socket");
  await run;
  assert.equal(store.get(id).op, null);
  assert.equal(store.get(id).status, "disconnected");
  assertAll(true, "terminal");
});

test("close requested before the initial registration stays gated through an early connected event", async () => {
  const { id, done } = startPending(22);
  await tick();
  hold("close_session");
  const run = sessions.close(id);
  emitConnected(id);
  assertAll(true, "early connected after close request");
  assert.equal(sessions.sendTarget(id), null);
  ipc.pending.get("connect_socket").resolve();
  ipc.pending.delete("connect_socket");
  await done;
  ipc.pending.get("close_session").resolve();
  ipc.pending.delete("close_session");
  await run;
  assert.equal(store.get(id), null);
});

test("rejected initial connect after a disconnect request: no backend disconnect, op cleared", async () => {
  const { id, done } = startPending(23);
  await tick();
  const run = sessions.disconnect(id);
  assert.equal(store.get(id).op, "disconnecting");
  const disconnects = countCalls("disconnect_socket");
  ipc.pending.get("connect_socket").reject(new Error("refused"));
  ipc.pending.delete("connect_socket");
  assert.equal((await done).ok, false);
  assert.deepEqual(await run, { ok: true });
  assert.equal(countCalls("disconnect_socket"), disconnects, "unknown id never sent to the backend");
  assert.equal(store.get(id).op, null);
  assert.equal(store.get(id).status, "error");
  assertAll(true, "errored session");
  // Idempotent: another disconnect on the terminal session is a no-op that leaves no op.
  assert.deepEqual(await sessions.disconnect(id), { ok: true });
  assert.equal(store.get(id).op, null);
});
