// Integration: the real connection.js with a Home form whose validity follows the browser
// rule (enabled fields only: required + min/max). Stale invalid values in the inactive
// mode must neither block the active mode nor let an invalid active port through.
import test from "node:test";
import assert from "node:assert/strict";

class FakeNode {
  constructor(id = "") {
    this.id = id;
    this.children = [];
    this.listeners = {};
    this.classes = new Set();
    this.dataset = {};
    this.value = "";
    this.disabled = false;
    this.required = false;
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
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  fire(type) {
    const ev = { preventDefault() {} };
    for (const fn of this.listeners[type] ?? []) fn(ev);
  }
  dispatchEvent() {}
  appendChild(c) {
    this.children.push(c);
    return c;
  }
  replaceChildren() {
    this.children = [];
  }
}

/** A port input that validates like the browser: skipped when disabled. */
const portField = (id) => {
  const f = new FakeNode(id);
  f.valid = () => {
    if (f.disabled) return true;
    if (f.value === "") return !f.required;
    const n = Number(f.value);
    return Number.isInteger(n) && n >= 1 && n <= 65535;
  };
  return f;
};

const nodes = new Map();
const getNode = (id) => {
  if (!nodes.has(id)) nodes.set(id, new FakeNode(id));
  return nodes.get(id);
};
const form = getNode("connect-form");
form.host = Object.assign(new FakeNode("host"), { valid: () => true });
form.host.valid = () => form.host.disabled || !form.host.required || form.host.value.trim() !== "";
form.port = portField("port");
form["server-port"] = portField("server-port");
form.invalidReports = 0;
form.reportValidity = () => {
  const ok = [form.host, form.port, form["server-port"]].every((f) => f.valid());
  if (!ok) form.invalidReports += 1;
  return ok;
};

const storage = new Map();
globalThis.localStorage = {
  getItem: (k) => storage.get(k) ?? null,
  setItem: (k, v) => storage.set(k, String(v)),
};
const winListeners = {};
const calls = [];
globalThis.document = {
  getElementById: getNode,
  createElement: () => new FakeNode(),
  querySelectorAll: () => [],
};
globalThis.window = {
  addEventListener: (type, fn) => (winListeners[type] ??= []).push(fn),
  __TAURI__: {
    core: {
      invoke: async (cmd, args) => {
        calls.push({ cmd, args });
      },
    },
    event: { listen: async () => {} },
  },
  connection_status: { get: () => "disconnected", set() {}, subscribe() {} },
};

await import("../src/connection.js");
for (const fn of winListeners.DOMContentLoaded) await fn();

const connects = () => calls.filter((c) => c.cmd === "connect_socket");
const tick = () => new Promise((r) => setTimeout(r, 0));
const submit = async () => {
  form.fire("submit");
  await tick();
};

test("initial state: client fields active, server field disabled", () => {
  assert.equal(form.host.disabled, false);
  assert.equal(form.port.disabled, false);
  assert.equal(form["server-port"].disabled, true);
  assert.equal(form["server-port"].required, false);
});

test("stale invalid client port does not block a valid server start", async () => {
  form.host.value = "10.0.0.1";
  form.port.value = "70000"; // invalid, then hidden by switching mode
  getNode("server-mode").fire("click");
  assert.equal(form.port.disabled, true);
  assert.equal(form.host.disabled, true);
  assert.equal(form["server-port"].disabled, false);
  assert.equal(form["server-port"].required, true);
  form["server-port"].value = "5001";
  const before = connects().length;
  await submit();
  assert.equal(connects().length, before + 1, "server connect invoked");
  assert.deepEqual(connects().at(-1).args.req, { type: "ServerStartRequest", port: 5001 });
  assert.equal(form.port.disabled, true, "still consistent after the submission settled");
  assert.equal(form["server-port"].disabled, false);
});

test("stale invalid server port does not block a valid client connect", async () => {
  getNode("server-mode").fire("click");
  form["server-port"].value = "0";
  getNode("client-mode").fire("click");
  assert.equal(form["server-port"].disabled, true);
  assert.equal(form.host.disabled, false);
  form.host.value = "localhost";
  form.port.value = "6000";
  const before = connects().length;
  await submit();
  assert.equal(connects().length, before + 1);
  assert.deepEqual(connects().at(-1).args.req, { type: "ClientConnectRequest", ip: "localhost", port: 6000 });
});

test("an invalid ACTIVE port is still rejected, whatever the hidden one holds", async () => {
  getNode("client-mode").fire("click");
  form["server-port"].value = "5001"; // valid but inactive: must not rescue the form
  form.host.value = "localhost";
  form.port.value = "70000";
  const before = connects().length;
  const reports = form.invalidReports;
  await submit();
  assert.equal(connects().length, before, "no connect with an invalid client port");
  assert.equal(form.invalidReports, reports + 1);

  getNode("server-mode").fire("click");
  form.port.value = "6000"; // valid but inactive
  form["server-port"].value = "99999";
  await submit();
  assert.equal(connects().length, before, "no start with an invalid server port");
});
