// Integration: the real connection.js renders the recent endpoints of Home as cards whose
// quiet "↗" arrow is paired with a verb (shown on hover by the CSS, always read by screen readers).
import test from "node:test";
import assert from "node:assert/strict";
import { installTemplates, bindStaticActions } from "./helpers/markup.js";

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
  set className(v) {
    this.classes = new Set(String(v).split(/\s+/).filter(Boolean));
  }
  get className() {
    return [...this.classes].join(" ");
  }
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  fire(type) {
    for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {} });
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

const nodes = new Map();
const getNode = (id) => {
  if (!nodes.has(id)) nodes.set(id, new FakeNode(id));
  return nodes.get(id);
};
const form = getNode("connect-form");
for (const name of ["host", "port", "server-port"]) form[name] = new FakeNode(name);

const storage = new Map([
  [
    "simautomate:recent-endpoints",
    JSON.stringify([
      { mode: "client", host: "10.0.0.1", port: 6000, lastUsed: 2 },
      { mode: "server", host: "", port: 5001, lastUsed: 1 },
    ]),
  ],
]);
globalThis.localStorage = {
  getItem: (k) => storage.get(k) ?? null,
  setItem: (k, v) => storage.set(k, String(v)),
  removeItem: (k) => storage.delete(k),
};
const winListeners = {};
globalThis.document = {
  getElementById: getNode,
  createElement: () => new FakeNode(),
  querySelectorAll: () => [],
};
globalThis.window = {
  addEventListener: (type, fn) => (winListeners[type] ??= []).push(fn),
  __TAURI__: { core: { invoke: async () => {} }, event: { listen: async () => {} } },
  connection_status: { get: () => "disconnected", set() {}, subscribe() {} },
};

await installTemplates(["home/recent-card.html", "home/recent-side-item.html"]);
await bindStaticActions("home/home.html", getNode);
await import("../src/home/connection.js");

const goOf = (card) => card.children[0].children.find((n) => n.classes.has("go"));

test("each recent card pairs the verb with the quiet arrow", () => {
  const cards = getNode("recent-list").children;
  assert.equal(cards.length, 2);

  const [client, server] = cards.map(goOf);
  assert.deepEqual(client.children.map((n) => n.textContent), ["Connect ", "↗"]);
  assert.equal(client.children[0].classes.has("verb"), true);
  assert.deepEqual(server.children.map((n) => n.textContent), ["Listen ", "↗"]);
  assert.equal(server.children[0].classes.has("verb"), true);
});

test("the server hint follows the selected mode", () => {
  assert.equal(getNode("connect-hint").classes.has("hidden"), true, "client mode by default");
  getNode("server-mode").fire("click");
  assert.equal(getNode("connect-hint").classes.has("hidden"), false);
  getNode("client-mode").fire("click");
  assert.equal(getNode("connect-hint").classes.has("hidden"), true);
});
