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
    this.attrs = {};
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
  setAttribute(name, value) {
    this.attrs[name] = String(value);
  }
  getAttribute(name) {
    return this.attrs[name] ?? null;
  }
  scrollIntoView() {}
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

await installTemplates(["home/recent-card.html", "home/recent-side-item.html"]);
await bindStaticActions("home/home.html", getNode);
await import("../src/home/connection.js");

// Each card is a wrapper: the card button first, then its copy button.
const cardButton = (card) => card.children[0];
const copyButton = (card) => card.children[1];
const goOf = (card) => cardButton(card).children[0].children.find((n) => n.classes.has("go"));

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
  const [client, , ssh] = cards.map((card) => cardButton(card).dataset);
  assert.deepEqual({ ...client }, { action: "click:home.useRecent", mode: "client", host: "10.0.0.1", port: 6000 });
  assert.equal(ssh.sshHost, "vm");
  assert.equal(ssh.sshPort, 22);
  assert.equal(ssh.sshUser, "lis");
  assert.equal("sshHost" in client, false);
  assert.deepEqual({ ...copyButton(cards[0]).dataset }, { action: "click:home.copyRecent", mode: "client", host: "10.0.0.1", port: 6000 });
});

test("the hint follows the selected mode", () => {
  assert.equal(getNode("connect-hint").classes.has("hidden"), true, "client mode by default");
  getNode("server-mode").fire("click");
  assert.equal(getNode("connect-hint").classes.has("hidden"), false);
  getNode("client-mode").fire("click");
  assert.equal(getNode("connect-hint").classes.has("hidden"), true);
});

const connectCalls = () => calls.filter((c) => c.cmd === "connect_socket");
const recentCard = (index) => getNode("recent-list").children[index];

test("the copy button names the endpoint it copies", () => {
  assert.equal(copyButton(recentCard(0)).getAttribute("aria-label"), "Copy 10.0.0.1:6000 to the connection form");
});

test("copy fills the form with a client endpoint and starts nothing", () => {
  getNode("server-mode").fire("click");
  getNode("connect-error").classes.delete("hidden");
  copyButton(recentCard(0)).fire("click");

  assert.equal(getNode("client-mode").classes.has("on"), true);
  assert.equal(getNode("server-mode").classes.has("on"), false);
  assert.equal(form.host.value, "10.0.0.1");
  assert.equal(String(form.port.value), "6000");
  assert.equal(getNode("connect-error").classes.has("hidden"), true, "a stale error is cleared");
  assert.deepEqual(connectCalls(), []);
});

test("copy fills the server port of a server endpoint", () => {
  copyButton(recentCard(1)).fire("click");

  assert.equal(getNode("server-mode").classes.has("on"), true);
  assert.equal(getNode("server-port-field").classes.has("hidden"), false);
  assert.equal(String(form["server-port"].value), "5001");
  assert.deepEqual(connectCalls(), []);
});

test("copy fills the SSH tunnel fields of an SSH client endpoint", () => {
  copyButton(recentCard(2)).fire("click");

  assert.equal(getNode("ssh-client-mode").classes.has("on"), true);
  assert.equal(getNode("ssh-fields").classes.has("hidden"), false);
  assert.deepEqual(
    {
      host: form.host.value,
      port: String(form.port.value),
      sshHost: form["ssh-host"].value,
      sshPort: String(form["ssh-port"].value),
      sshUser: form["ssh-user"].value,
    },
    { host: "10.0.0.2", port: "2575", sshHost: "vm", sshPort: "22", sshUser: "lis" },
  );
  assert.deepEqual(connectCalls(), []);
});

test("copy clears the SSH user when the endpoint has none", () => {
  form["ssh-user"].value = "lis";
  copyButton(recentCard(3)).fire("click");

  assert.equal(getNode("ssh-server-mode").classes.has("on"), true);
  assert.equal(String(form["server-port"].value), "5002");
  assert.equal(form["ssh-host"].value, "vm");
  assert.equal(String(form["ssh-port"].value), "2222");
  assert.equal(form["ssh-user"].value, "");
  assert.deepEqual(connectCalls(), []);
});

test("the card still opens a session, unlike its copy button", async () => {
  cardButton(recentCard(0)).fire("click");
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(connectCalls().length, 1);
});
