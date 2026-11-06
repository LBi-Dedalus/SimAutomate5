// Integration: the template split Send button (real sessions, messages and templates modules on a
// fake DOM): quick send, menu choice, unavailable sessions, keyboard, and no redirection.
import test from "node:test";
import assert from "node:assert/strict";
import { installTemplates, bindStaticActions } from "./helpers/markup.js";

class FakeNode {
  constructor(tag = "div", id = "") {
    this.tag = tag;
    this.id = id;
    this.children = [];
    this.listeners = {};
    this.classes = new Set();
    this.dataset = {};
    this.style = {};
    this.attrs = {};
    this.value = "";
    this.disabled = false;
    this.textContent = "";
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
  fire(type, event = {}) {
    for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {}, stopPropagation() {}, target: this, ...event });
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
  setAttribute(name, value) {
    this.attrs[name] = String(value);
  }
  focus() {
    document.activeElement = this;
  }
  select() {}
  scrollIntoView() {}
  toggleAttribute() {}
  contains(node) {
    return node === this || this.children.some((c) => c.contains?.(node));
  }
  getBoundingClientRect() {
    return { top: 500, bottom: 530, left: 600, right: 640 };
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
const getNode = (id) => {
  if (!nodes.has(id)) nodes.set(id, new FakeNode("div", id));
  return nodes.get(id);
};
const composerForm = getNode("message-form");
composerForm.message = new FakeNode("textarea", "message");
composerForm.querySelector = () => new FakeNode("button");

globalThis.document = {
  activeElement: null,
  getElementById: getNode,
  createElement: (tag) => new FakeNode(tag),
  createTextNode: (t) => ({ textContent: t }),
  querySelectorAll(sel) {
    if (sel === "#tpl-vars input") return getNode("tpl-vars").find((n) => n.tag === "input");
    return [];
  },
  addEventListener: (type, fn) => (docListeners[type] ??= []).push(fn),
  removeEventListener: (type, fn) => {
    docListeners[type] = (docListeners[type] ?? []).filter((f) => f !== fn);
  },
  dispatchEvent: () => {},
};
const ipc = { calls: [], failSend: false, buildGate: null };
globalThis.window = {
  innerWidth: 1000,
  innerHeight: 700,
  addEventListener() {},
  removeEventListener() {},
  __TAURI__: {
    core: {
      invoke: async (cmd, args) => {
        ipc.calls.push({ cmd, args });
        if (cmd === "load_config") {
          return {
            revision: 0,
            templates: [
              { id: "a", name: "Alpha", description: "", payload: "ALPHA", variables: [] },
              { id: "z", name: "Zulu", description: "", payload: "ZULU", variables: [], autobuild: true },
            ],
            auto_reply: { enabled: true, rules: [] },
          };
        }
        if (cmd === "auto_build_message_cmd") {
          await ipc.buildGate;
          return "BUILT";
        }
        if (cmd === "send_message" && ipc.failSend) throw new Error("refused");
        return undefined;
      },
    },
    event: { listen: async () => {} },
  },
  connection_status: { get: () => "disconnected", set() {}, subscribe() {} },
};

await installTemplates(["templates/template-card.html", "templates/variable-auto-row.html", "templates/variable-value-row.html", "session/system-message.html", "session/message-bubble.html"]);
await bindStaticActions("templates/templates.html", getNode);
const { sessions, store } = await import("../src/session/sessions.js");
await import("../src/session/messages.js");
await import("../src/templates/templates.js");
const { showView, currentView } = await import("../src/nav.js");
// A successful send now opens the Session view: tests that do not care go back to Templates.
const backToTemplates = () => {
  showView("templates", { force: true });
  store.select(a);
};

const tick = () => new Promise((r) => setTimeout(r, 0));
const startConnected = async (port, label, extra = {}) => {
  const { id, done } = sessions.start({ mode: "client", label, req: { type: "ClientConnectRequest", ip: "h", port }, ...extra });
  await done;
  sessions.handleStatus({ session_id: id, attempt: store.get(id).attempt, status: "connected" });
  return id;
};
const sent = () => ipc.calls.filter((c) => c.cmd === "send_message").map((c) => ({ id: c.args.sessionId, message: c.args.payload.message }));
const arrow = () => getNode("tpl-send-menu");
const menu = () => getNode("tpl-send-list");
const entries = () => menu().children;
const entryFor = (id) => entries().find((b) => b.dataset.sessionId === id);
const pressKey = (key) => {
  for (const fn of [...(docListeners.keydown ?? [])]) fn({ key, preventDefault() {}, stopPropagation() {} });
};

// Open the template in the editor.
const card = getNode("tpl-list").children[0];
card.find((n) => n.classes.has("tload"))[0].fire("click");
showView("templates", { force: true });

let a, b, c;
test("main button says Send to <name>, falls back to Send, and follows renames", async () => {
  assert.equal(getNode("tpl-send").textContent, "Send");
  assert.equal(getNode("tpl-send").disabled, true);
  assert.equal(arrow().disabled, true, "no session can send");
  a = await startConnected(1, "Alpha");
  b = await startConnected(2, "Beta");
  store.select(a);
  assert.equal(getNode("tpl-send").textContent, "Send to Alpha");
  store.select(b);
  assert.equal(getNode("tpl-send").textContent, "Send to Beta");
  store.get(b).label = "Beta 2";
  store.relabel({ mode: "none" }, "x"); // emits "renamed"
  assert.equal(getNode("tpl-send").textContent, "Send to Beta 2");
  store.select(a);
});

test("quick send goes to the selected session", async () => {
  ipc.calls = [];
  getNode("tpl-send").fire("click");
  await tick();
  assert.deepEqual(sent(), [{ id: a, message: "ALPHA" }]);
  assert.equal(currentView(), "session", "quick send opens the current session");
  assert.equal(store.activeId, a);
  backToTemplates();
});

test("menu lists sessions with state, marks the current one, disables those that cannot send", async () => {
  c = sessions.start({ mode: "client", label: "Gamma", req: { type: "ClientConnectRequest", ip: "h", port: 3 } }).id;
  await tick();
  store.select(a); // starting a session selects it
  arrow().fire("click");
  assert.equal(menu().hidden, false);  assert.equal(arrow().attrs["aria-expanded"], "true");
  assert.equal(entries().length, 3);
  assert.equal(entryFor(a).disabled, false);
  assert.equal(entryFor(c).disabled, true, "connecting session cannot be chosen");
  assert.match(entryFor(a).attrs["aria-label"], /Alpha \(current\): Connected/);
  assert.equal(entryFor(a).attrs.role, "menuitem");
  assert.equal(document.activeElement, entryFor(a), "focus starts on the current session");
  // A disabled entry never sends, even if clicking is simulated.
  ipc.calls = [];
  entryFor(c).fire("click");
  await tick();
  assert.deepEqual(sent(), [], "unavailable session cannot be chosen");
  pressKey("Escape");
});

test("choosing a session sends immediately to that session and closes the menu", async () => {
  ipc.calls = [];
  arrow().fire("click");
  entryFor(b).fire("click");
  assert.equal(menu().hidden, true);
  assert.equal(store.activeId, a, "choosing alone does not select");
  assert.equal(currentView(), "templates", "nor switch the view before the send completes");
  await tick();
  assert.deepEqual(sent(), [{ id: b, message: "ALPHA" }]);
  assert.equal(store.activeId, b, "the destination is selected after the send");
  assert.equal(currentView(), "session");
  backToTemplates();
});

test("keyboard: arrows skip disabled entries, Escape and Tab close, Escape restores focus to the arrow", async () => {
  arrow().fire("click");
  assert.equal(document.activeElement, entryFor(a));
  pressKey("ArrowDown");
  assert.equal(document.activeElement, entryFor(b));
  pressKey("ArrowDown");
  assert.equal(document.activeElement, entryFor(a), "wraps over the disabled session");
  pressKey("End");
  assert.equal(document.activeElement, entryFor(b));
  pressKey("Home");
  assert.equal(document.activeElement, entryFor(a));
  pressKey("Escape");
  assert.equal(menu().hidden, true);
  assert.equal(arrow().attrs["aria-expanded"], "false");
  assert.equal(document.activeElement, arrow());
  assert.equal(docListeners.keydown?.length ?? 0, 0, "listeners removed on close");
  arrow().fire("click");
  pressKey("Tab");
  assert.equal(menu().hidden, true);
});

test("an outside click closes the menu, an inside one does not", () => {
  arrow().fire("click");
  for (const fn of docListeners.pointerdown) fn({ target: entryFor(a) });
  assert.equal(menu().hidden, false);
  for (const fn of docListeners.pointerdown) fn({ target: new FakeNode("div") });
  assert.equal(menu().hidden, true);
});

test("the menu stays usable when the current session cannot send but another can", () => {
  store.select(c);
  assert.equal(getNode("tpl-send").disabled, true);
  assert.equal(arrow().disabled, false);
  assert.equal(getNode("tpl-send").textContent, "Send to Gamma");
  arrow().fire("click");
  assert.equal(document.activeElement, entryFor(a), "first enabled session when the current is disabled");
  assert.match(entryFor(c).attrs["aria-label"], /Gamma \(current\)/);
  pressKey("Escape");
  store.select(a);
});

test("session changes while the menu is open update it and never redirect a send", async () => {
  arrow().fire("click");
  assert.equal(entryFor(b).disabled, false);
  sessions.handleStatus({ session_id: b, attempt: store.get(b).attempt, status: "disconnected" });
  assert.equal(entries().length, 3);
  assert.equal(entryFor(b).disabled, true, "refreshed in place");
  ipc.calls = [];
  store.select(b); // selection moves while open: the entries keep their own ids
  entryFor(a).fire("click");
  await tick();
  assert.deepEqual(sent(), [{ id: a, message: "ALPHA" }]);
  backToTemplates();
});

const openCard = (id) => {
  const el = getNode("tpl-list").children.find((n) => n.dataset.id === id);
  el.find((n) => n.classes.has("tload"))[0].fire("click");
};

test("unsaved template edits remain after a successful send that opens the session", async () => {
  getNode("tpl-name").value = "Alpha edited";
  getNode("tpl-name").fire("input");
  assert.equal(getNode("tpl-dirty").hidden, false);
  getNode("tpl-send").fire("click");
  await tick();
  assert.equal(currentView(), "session");
  assert.equal(getNode("tpl-name").value, "Alpha edited");
  assert.equal(getNode("tpl-dirty").hidden, false, "still dirty");
  backToTemplates();
  assert.equal(getNode("tpl-name").value, "Alpha edited");
  getNode("tpl-name").value = "Alpha";
  getNode("tpl-name").fire("input");
  assert.equal(getNode("tpl-dirty").hidden, true, "clean again");
});

test("a failed send stays in Templates with the banner", async () => {
  ipc.failSend = true;
  const before = store.activeId;
  getNode("tpl-send").fire("click");
  await tick();
  ipc.failSend = false;
  assert.equal(currentView(), "templates");
  assert.equal(store.activeId, before);
  assert.match(getNode("tpl-banner-text").textContent, /Sending to Alpha failed/);
});

test("moving to another view during preparation is respected", async () => {
  openCard("z"); // autobuild template: preparation awaits the build
  let release;
  ipc.buildGate = new Promise((r) => (release = r));
  const d = await startConnected(4, "Delta");
  store.select(a);
  ipc.calls = [];
  arrow().fire("click");
  entryFor(d).fire("click");
  await tick();
  showView("home", { force: true });
  release();
  await tick();
  assert.deepEqual(sent(), [{ id: d, message: "BUILT" }]);
  assert.equal(currentView(), "home");
  assert.equal(store.activeId, a);
  ipc.buildGate = null;
  backToTemplates();
});

test("selecting another template during preparation does not switch", async () => {
  let release;
  ipc.buildGate = new Promise((r) => (release = r));
  getNode("tpl-send").fire("click");
  await tick();
  getNode("tpl-new").fire("click"); // clean draft: a new template replaces it
  release();
  await tick();
  assert.equal(currentView(), "templates");
  assert.equal(store.activeId, a);
  ipc.buildGate = null;
  openCard("z");
});

test("a destination closed during preparation does not switch nor open another session", async () => {
  const e = await startConnected(5, "Epsilon");
  store.select(a);
  let release;
  ipc.buildGate = new Promise((r) => (release = r));
  ipc.calls = [];
  arrow().fire("click");
  entryFor(e).fire("click");
  await tick();
  store.remove(e);
  assert.equal(store.get(e), null);
  const activeAfterClose = store.activeId;
  release();
  await tick();
  assert.equal(currentView(), "templates");
  assert.equal(store.activeId, activeAfterClose, "no other session is opened");
  ipc.buildGate = null;
});

test("a failed menu send shows the error banner with the session name", async () => {
  ipc.failSend = true;
  arrow().fire("click");
  entryFor(a).fire("click");
  await tick();
  ipc.failSend = false;
  assert.match(getNode("tpl-banner-text").textContent, /Sending to Alpha failed/);
});

test("when no session can send the arrow is disabled and the menu closes", async () => {
  arrow().fire("click");
  sessions.handleStatus({ session_id: a, attempt: store.get(a).attempt, status: "disconnected" });
  for (const s of store.list()) sessions.handleStatus({ session_id: s.id, attempt: s.attempt, status: "disconnected" });
  assert.equal(arrow().disabled, true);
  assert.equal(menu().hidden, true);
  arrow().fire("click");
  assert.equal(menu().hidden, true, "cannot open");
});
