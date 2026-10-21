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
for (const name of ["host", "port", "server-port", "ssh-host", "ssh-port", "ssh-user"]) form[name] = new FakeNode(name);

const storage = new Map([
  [
    "simautomate:recent-endpoints",
    JSON.stringify([
      { mode: "client", host: "10.0.0.1", port: 6000, lastUsed: 4 },
      { mode: "server", host: "", port: 5001, lastUsed: 3 },
      { mode: "ssh-client", host: "10.0.0.2", port: 2575, sshHost: "vm", sshPort: 22, sshUser: "lis", lastUsed: 2 },
      { mode: "ssh-server", host: "", port: 5002, sshHost: "vm", sshPort: 2222, sshUser: "", lastUsed: 1 },
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
  assert.equal(cards.length, 4);

  const [client, server, sshClient, sshServer] = cards.map(goOf);
  assert.deepEqual(client.children.map((n) => n.textContent), ["Connect ", "↗"]);
  assert.equal(client.children[0].classes.has("verb"), true);
  assert.deepEqual(server.children.map((n) => n.textContent), ["Listen ", "↗"]);
  assert.equal(server.children[0].classes.has("verb"), true);
  assert.equal(sshClient.children[0].textContent, "Connect ");
  assert.equal(sshServer.children[0].textContent, "Listen ");
});

const slot = (node, name) => {
  const found = [];
  const visit = (n) => {
    for (const c of n.children) {
      if (c.dataset?.slot === name) found.push(c);
      visit(c);
    }
  };
  visit(node);
  return found[0];
};

test("recent cards show the mode and the SSH tunnel, and carry it for reconnecting", () => {
  const cards = getNode("recent-list").children;
  assert.deepEqual(
    cards.map((c) => [slot(c, "kind").textContent, slot(c, "endpoint").textContent]),
    [
      ["Client", "10.0.0.1:6000"],
      ["Server", ":5001"],
      ["SSH Client", "lis@vm:22 → 10.0.0.2:2575"],
      ["SSH Server", ":5002 ← vm:2222"],
    ],
  );
  assert.deepEqual({ ...cards[0].dataset }, { action: "click:home.useRecent", mode: "client", host: "10.0.0.1", port: 6000 });
  assert.equal(cards[2].dataset.sshHost, "vm");
  assert.equal(cards[2].dataset.sshPort, 22);
  assert.equal(cards[2].dataset.sshUser, "lis");
  assert.equal("sshHost" in cards[0].dataset, false);
});

test("the hint follows the selected mode", () => {
  assert.equal(getNode("connect-hint").classes.has("hidden"), true, "client mode by default");
  getNode("server-mode").fire("click");
  assert.equal(getNode("connect-hint").classes.has("hidden"), false);
  getNode("client-mode").fire("click");
  assert.equal(getNode("connect-hint").classes.has("hidden"), true);
});
