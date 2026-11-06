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
    this.style = {};
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
  replaceChildren(...nodes) {
    this.children = nodes;
  }
  setAttribute(name, value) {
    this.attrs[name] = String(value);
  }
  removeAttribute(name) {
    delete this.attrs[name];
  }
  getBoundingClientRect() {
    return { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 };
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

// Each card is a wrapper: the card button first, then a footer holding rename and copy (in that order).
const cardButton = (card) => card.children[0];
const footerButtons = (card) => card.children[1].children;
const renameButton = (card) => card.querySelector(".card-rename");
const copyButton = (card) => card.querySelector(".card-copy");
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
    cards.map((c) => [slot(c, "kind").textContent, slot(c, "name").textContent]),
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

test("the footer keeps the keyboard order: connect, then Rename, then Copy", () => {
  const card = recentCard(0);
  assert.equal(card.children.length, 2);
  assert.deepEqual(
    [...footerButtons(card)].map((b) => b.dataset.action),
    ["click:home.renameRecent", "click:home.copyRecent"],
  );
});

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

test("unnamed cards use the address as the title, without repeating it; a rename button carries the endpoint", () => {
  const card = recentCard(0);
  assert.equal(slot(card, "name").textContent, "10.0.0.1:6000", "the address is the title");
  assert.equal(slot(card, "name").classes.has("is-address"), true);
  assert.equal(slot(card, "endpoint").textContent, "", "no duplicate supporting line");
  assert.equal(renameButton(card).dataset.action, "click:home.renameRecent");
  assert.equal(renameButton(card).dataset.host, "10.0.0.1");
  assert.equal(renameButton(card).getAttribute("aria-label"), "Rename 10.0.0.1:6000");
});

test("sidebar action buttons sit in one stack, each a real button next to (not inside) the row button", () => {
  const item = getNode("side-recent").children[0];
  assert.equal(item.children[0].classes.has("ses"), true);
  const stack = item.children[1];
  assert.equal(stack.classes.has("ses-actions"), true);
  assert.deepEqual(stack.children.map((b) => b.classes.has("ses-rename")), [true]);
});

test("a saved name shows on the card and the sidebar item next to the address, and keeps the order", async () => {
  const { sessions } = await import("../src/session/sessions.js");
  const before = storage.get("simautomate:recent-endpoints");
  assert.equal(sessions.renameConnection({ mode: "client", host: "10.0.0.1", port: 6000 }, " Lab LIS ").ok, true);
  assert.equal(storage.get("simautomate:recent-endpoints"), before, "recent entries are untouched");

  const card = recentCard(0);
  assert.equal(slot(card, "name").textContent, "Lab LIS");
  assert.equal(slot(card, "endpoint").textContent, "10.0.0.1:6000");
  assert.equal(renameButton(card).getAttribute("aria-label"), "Rename Lab LIS");
  assert.deepEqual(getNode("recent-list").children.map((c) => slot(c, "name").textContent).slice(0, 2), ["Lab LIS", ":5001"]);

  const side = getNode("side-recent").children[0];
  assert.equal(slot(side, "endpoint").textContent, "Lab LIS");
  assert.match(slot(side, "detail").textContent, /^10\.0\.0\.1:6000 · Client/);

  // A save failure shows nothing as renamed.
  const failing = globalThis.localStorage.setItem;
  globalThis.localStorage.setItem = () => {
    throw new Error("full");
  };
  assert.equal(sessions.renameConnection({ mode: "client", host: "10.0.0.1", port: 6000 }, "Other").ok, false);
  globalThis.localStorage.setItem = failing;
  assert.equal(slot(recentCard(0), "name").textContent, "Lab LIS");
});

test("recent cards and sidebar items show styled details with the real address, even once renamed", () => {
  const rowsText = (anchor) => {
    anchor.fire("mouseenter");
    const tip = getNode("conn-tooltip");
    const out = tip.children[0].children.map((n) => n.textContent);
    anchor.fire("mouseleave");
    return out;
  };
  for (const anchor of [recentCard(0), getNode("side-recent").children[0]]) {
    assert.equal(rowsText(anchor).join("|"), "Name|Lab LIS|Type|Client|Host|10.0.0.1|Port|6000");
  }
  assert.equal(getNode("side-recent").children[0].querySelector(".ses").title, "", "no native tooltip");
  assert.equal(getNode("conn-tooltip").classes.has("show"), false);
});

test("card and sidebar action buttons show a text-only hint that replaces the row tooltip", () => {
  const tip = getNode("conn-tooltip");
  const card = recentCard(0);
  const side = getNode("side-recent").children[0];
  for (const [row, button, text] of [
    [card, copyButton(card), "Copy this configuration to the connection form"],
    [card, renameButton(card), "Rename this connection"],
    [side, side.querySelector(".ses-rename"), "Rename"],
  ]) {
    assert.equal(button.title, "", "no native title");
    assert.equal(button.getAttribute("data-tooltip") ?? undefined, undefined);
    row.fire("mouseenter");
    assert.equal(tip.classes.has("text"), false, "row details first");
    button.fire("mouseenter");
    assert.equal(tip.classes.has("text"), true);
    assert.equal(tip.children[0].textContent, text);
    button.fire("mouseleave");
    assert.equal(tip.classes.has("text"), false, "row details come back");
    row.fire("mouseleave");
    assert.equal(tip.classes.has("show"), false);
  }
});

test("a new session from a named recent card takes the saved name", async () => {
  const { store } = await import("../src/session/sessions.js");
  cardButton(recentCard(0)).fire("click");
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(store.active().label, "Lab LIS");
});
