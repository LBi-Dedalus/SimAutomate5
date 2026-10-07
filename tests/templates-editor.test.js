// Integration tests for src/templates.js driven through a minimal fake DOM and a
// mocked Tauri IPC. They exercise the real module: events, guards, rendering.
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
    this.value = "";
    this.readOnly = false;
    this.disabled = false;
    this.textContent = "";
    this.selectionStart = 0;
    this.selectionEnd = 0;
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
  fire(type) {
    for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {} });
  }
  appendChild(c) {
    this.children.push(c);
    return c;
  }
  replaceChildren() {
    this.children = [];
  }
  setAttribute() {}
  focus() {}
  select() {}
  setRangeText(text, start, end) {
    this.value = this.value.slice(0, start) + text + this.value.slice(end);
    this.selectionStart = this.selectionEnd = start + text.length;
  }
  /** Simulates the user typing into a field. */
  type(value) {
    this.value = value;
    this.fire("input");
  }
  find(pred, out = []) {
    for (const c of this.children) {
      if (pred(c)) out.push(c);
      c.find?.(pred, out);
    }
    return out;
  }
  get text() {
    return this.textContent + this.children.map((c) => c.text ?? c.textContent ?? "").join("");
  }
}

const nodes = new Map();
const docListeners = {};
const winListeners = {};
const getNode = (id) => {
  if (!nodes.has(id)) nodes.set(id, new FakeNode("div", id));
  return nodes.get(id);
};
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
const ipc = { handler: async () => ({}), calls: [] };
globalThis.window = {
  addEventListener: (type, fn) => (winListeners[type] ??= []).push(fn),
  __TAURI__: {
    core: {
      invoke: (cmd, args) => {
        ipc.calls.push({ cmd, args });
        return ipc.handler(cmd, args);
      },
    },
    event: { listen: async () => {} },
  },
  connection_status: { get: () => "disconnected", subscribe() {} },
};

const SEED = [
  { id: "a", name: "Alpha", description: "", payload: "ALPHA {{X}}", variables: [{ name: "X", default: "1" }] },
  { id: "b", name: "Beta", description: "", payload: "BETA", variables: [] },
];

const { showView, currentView } = await import("../src/nav.js");
await import("../src/templates.js");

/** Deferred promise helper. */
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((res, rej) => ((resolve = res), (reject = rej)));
  return { promise, resolve, reject };
};
const tick = () => new Promise((r) => setTimeout(r, 0));

let booted = false;
async function boot(stored = SEED) {
  ipc.calls = [];
  ipc.handler = async (cmd) => (cmd === "load_templates" ? { templates: structuredClone(stored), seeded: false } : undefined);
  if (!booted) {
    booted = true;
    for (const fn of winListeners.DOMContentLoaded ?? []) {
      try {
        await fn();
      } catch {
        /* other modules' init needs real DOM; irrelevant here */
      }
    }
    for (const fn of docListeners.DOMContentLoaded ?? []) {
      try {
        await fn();
      } catch {
        /* same */
      }
    }
  } else {
    getNode("tpl-banner-retry").fire("click"); // reload from the mocked IPC
    await tick();
  }
  // Return to a clean state: no guard, nothing selected.
  getNode("tpl-guard-cancel").fire("click");
}

const cards = () => getNode("tpl-list").children;
const cardNamed = (name) => cards().find((c) => c.text.includes(name));
const open = (name) => cardNamed(name).fire("click");
const form = () => getNode("tpl-form");
const guard = () => getNode("tpl-guard");
const savedOk = () => ipc.calls.filter((c) => c.cmd === "save_templates");

test("deferred initial load: New/Save/shortcuts are refused and the persisted collection is untouched", async () => {
  const gate = deferred();
  ipc.calls = [];
  ipc.handler = (cmd) => (cmd === "load_templates" ? gate.promise : Promise.resolve());
  booted = true;
  const inits = [...(winListeners.DOMContentLoaded ?? []), ...(docListeners.DOMContentLoaded ?? [])];
  const pending = inits.map((fn) => Promise.resolve().then(fn).catch(() => {}));
  await tick();

  // Loading: no collection yet, editing disabled, empty != unknown.
  assert.equal(getNode("tpl-new").disabled, true);
  assert.match(getNode("tpl-list").text, /Loading/);
  getNode("tpl-new").fire("click");
  assert.equal(form().hidden, true, "New is refused while loading");
  assert.equal(getNode("tpl-new").disabled, true);
  getNode("tpl-save").fire("click");
  assert.equal(savedOk().length, 0, "no write while loading");

  // "Save message as template" requests are explicitly refused, never queued.
  for (const p of ["FIRST", "SECOND", "THIRD"]) {
    for (const fn of docListeners["simautomate:save-as-template"] ?? []) fn({ detail: { payload: p } });
    assert.match(getNode("tpl-banner-text").textContent, /still loading.*repeat the action.*not kept/);
  }

  gate.resolve({ templates: structuredClone(SEED), seeded: false });
  await Promise.all(pending);
  await tick();
  assert.equal(cards().length, 2, "no refused request executed automatically");
  assert.equal(form().hidden, true, "no draft created");
  assert.equal(savedOk().length, 0);
  // User retry after loading succeeds.
  for (const fn of docListeners["simautomate:save-as-template"] ?? []) fn({ detail: { payload: "KEPT PAYLOAD" } });
  assert.equal(cards().length, 3, "2 persisted + the new draft");
  assert.equal(getNode("tpl-payload").value, "KEPT PAYLOAD");
  // Saving appends to the loaded collection instead of replacing it.
  getNode("tpl-name").type("Kept");
  getNode("tpl-save").fire("click");
  await tick();
  assert.deepEqual(
    savedOk().at(-1).args.templates.map((t) => t.id).slice(0, 2),
    ["a", "b"],
  );
  assert.equal(savedOk().at(-1).args.templates.length, 3);
  getNode("tpl-delete").fire("click");
  getNode("tpl-delete").fire("click");
  await tick();
});

test("failed load refuses writes and requests; retry then user repeat succeeds; overlapping retries are shared", async () => {
  ipc.calls = [];
  ipc.handler = async () => {
    throw "boom";
  };
  getNode("tpl-banner-retry").fire("click");
  await tick();
  assert.equal(getNode("tpl-new").disabled, true);
  assert.equal(getNode("tpl-banner-retry").hidden, false);
  getNode("tpl-new").fire("click");
  assert.equal(form().hidden, true);
  assert.equal(savedOk().length, 0);
  for (const fn of docListeners["simautomate:save-as-template"] ?? []) fn({ detail: { payload: "RETRY ME" } });
  assert.match(getNode("tpl-banner-text").textContent, /Retry loading.*repeat the action.*not kept/);
  assert.equal(form().hidden, true);

  const gate = deferred();
  ipc.handler = (cmd) => (cmd === "load_templates" ? gate.promise : Promise.resolve());
  ipc.calls = [];
  getNode("tpl-banner-retry").fire("click");
  getNode("tpl-banner-retry").fire("click");
  assert.equal(ipc.calls.filter((c) => c.cmd === "load_templates").length, 1, "no overlapping loads");
  gate.resolve({ templates: structuredClone(SEED), seeded: false });
  await tick();
  assert.equal(form().hidden, true, "refused request not replayed after retry");
  assert.equal(getNode("tpl-new").disabled, false);
  for (const fn of docListeners["simautomate:save-as-template"] ?? []) fn({ detail: { payload: "RETRY ME" } });
  assert.equal(getNode("tpl-payload").value, "RETRY ME");
  assert.equal(savedOk().length, 0);
  getNode("tpl-delete").fire("click"); // discard the unsaved draft (new)
  getNode("tpl-delete").fire("click");
});

test("a pending reload cannot clobber a draft and writes wait for it", async () => {
  await boot();
  getNode("tpl-new").fire("click");
  getNode("tpl-name").type("Draft X");
  const gate = deferred();
  ipc.handler = (cmd) => (cmd === "load_templates" ? gate.promise : Promise.resolve());
  ipc.calls = [];
  getNode("tpl-banner-retry").fire("click"); // reload starts
  getNode("tpl-save").fire("click");
  assert.equal(savedOk().length, 0, "write refused during reload");
  assert.equal(getNode("tpl-save").disabled, true);
  gate.resolve({ templates: structuredClone(SEED), seeded: false });
  await tick();
  assert.equal(getNode("tpl-name").value, "Draft X", "dirty draft kept");
  getNode("tpl-save").fire("click");
  await tick();
  const saved = savedOk().at(-1).args.templates;
  assert.equal(saved.length, 3);
  // A reload requested during a pending save is not applied over it.
  getNode("tpl-name").type("Draft Y");
  const wgate = deferred();
  ipc.handler = () => wgate.promise;
  ipc.calls = [];
  getNode("tpl-save").fire("click");
  getNode("tpl-banner-retry").fire("click");
  assert.equal(ipc.calls.filter((c) => c.cmd === "load_templates").length, 0);
  wgate.resolve();
  await tick();
  assert.equal(getNode("tpl-dirty").hidden, true);
  getNode("tpl-delete").fire("click");
  getNode("tpl-delete").fire("click");
  await tick();
});

test("setup: loads and opens a template", async () => {
  await boot();
  assert.equal(cards().length, 2);
  open("Alpha");
  assert.equal(getNode("tpl-payload").value, "ALPHA {{X}}");
  assert.equal(form().hidden, false);
});

test("pending save locks the draft: edits and selection cannot be lost", async () => {
  await boot();
  open("Alpha");
  getNode("tpl-payload").type("ALPHA changed");
  const gate = deferred();
  ipc.handler = () => gate.promise;
  getNode("tpl-save").fire("click");
  assert.equal(getNode("tpl-save").disabled, true);
  assert.equal(getNode("tpl-payload").readOnly, true);

  // Typing during the pending save is rejected and the field is restored.
  getNode("tpl-payload").type("ALPHA changed AND MORE");
  getNode("tpl-name").type("Hijack");
  assert.equal(getNode("tpl-payload").value, "ALPHA changed");
  assert.equal(getNode("tpl-name").value, "Alpha");

  // Selection / new are refused while pending.
  cardNamed("Beta").fire("click");
  getNode("tpl-new").fire("click");
  assert.equal(getNode("tpl-payload").value, "ALPHA changed");

  // Programmatic navigation is held too.
  showView("session", { force: false });
  assert.equal(currentView(), "templates");

  gate.resolve();
  await tick();
  assert.equal(getNode("tpl-dirty").hidden, true, "saved content is clean");
  assert.equal(getNode("tpl-payload").readOnly, false);
  assert.equal(savedOk().at(-1).args.templates.find((t) => t.id === "a").payload, "ALPHA changed");

  // Edits after completion are dirty again.
  getNode("tpl-payload").type("ALPHA later");
  assert.equal(getNode("tpl-dirty").hidden, false);
});

test("guarded navigation does not run on save success until save resolves, and edits stay dirty", async () => {
  await boot();
  open("Alpha");
  getNode("tpl-payload").type("ALPHA nav");
  showView("home"); // blocked by guard
  assert.equal(currentView(), "templates");
  assert.equal(guard().hidden, false);
  const gate = deferred();
  ipc.handler = () => gate.promise;
  getNode("tpl-guard-save").fire("click");
  getNode("tpl-payload").type("sneaky"); // locked
  getNode("tpl-guard-discard").fire("click"); // ignored while saving
  assert.equal(currentView(), "templates");
  assert.equal(getNode("tpl-payload").value, "ALPHA nav");
  gate.resolve();
  await tick();
  assert.equal(currentView(), "home");
  assert.equal(guard().hidden, true);
  showView("templates", { force: true });
  assert.equal(getNode("tpl-dirty").hidden, true);
  assert.equal(getNode("tpl-payload").value, "ALPHA nav");
});

test("guard Cancel is disabled/refused while its save is pending; failed save keeps guard usable", async () => {
  await boot();
  open("Alpha");
  getNode("tpl-payload").type("ALPHA guarded");
  showView("home");
  assert.equal(guard().hidden, false);
  const gate = deferred();
  ipc.handler = () => gate.promise;
  getNode("tpl-guard-save").fire("click");
  assert.equal(getNode("tpl-guard-cancel").disabled, true);
  getNode("tpl-guard-cancel").fire("click");
  assert.equal(guard().hidden, false, "cancel refused while saving");
  showView("session"); // cannot replace the guarded action while busy
  assert.equal(currentView(), "templates");
  gate.resolve();
  await tick();
  assert.equal(currentView(), "home", "original continuation runs");
  assert.equal(guard().hidden, true);
  assert.equal(getNode("tpl-guard-cancel").disabled, false);

  // Failing save: guard stays open, unlocked, and Cancel works afterwards.
  showView("templates", { force: true });
  getNode("tpl-payload").type("ALPHA again");
  showView("home");
  assert.equal(guard().hidden, false);
  ipc.handler = async () => {
    throw "disk full";
  };
  getNode("tpl-guard-save").fire("click");
  await tick();
  assert.equal(currentView(), "templates");
  assert.equal(guard().hidden, false);
  assert.equal(getNode("tpl-guard-cancel").disabled, false);
  assert.equal(getNode("tpl-save").disabled, false);
  assert.equal(getNode("tpl-payload").readOnly, false);
  getNode("tpl-guard-cancel").fire("click");
  assert.equal(guard().hidden, true);
  assert.equal(currentView(), "templates");
  assert.equal(getNode("tpl-dirty").hidden, false);
});

test("failed save keeps the draft dirty, shows the error, and can be retried", async () => {
  await boot();
  open("Alpha");
  getNode("tpl-payload").type("ALPHA fail");
  ipc.handler = async () => {
    throw "disk full";
  };
  getNode("tpl-save").fire("click");
  await tick();
  assert.equal(getNode("tpl-dirty").hidden, false);
  assert.equal(getNode("tpl-save").disabled, false);
  assert.equal(getNode("tpl-delete").disabled, false);
  assert.equal(getNode("tpl-payload").readOnly, false);
  assert.match(getNode("tpl-banner-text").textContent, /disk full/);
  ipc.handler = async () => undefined;
  getNode("tpl-save").fire("click");
  await tick();
  assert.equal(getNode("tpl-dirty").hidden, true);
});

test("discarding a saved draft visibly restores payload and preview", async () => {
  await boot();
  open("Alpha");
  getNode("tpl-payload").type("ALPHA edited");
  showView("home");
  assert.equal(guard().hidden, false);
  getNode("tpl-guard-discard").fire("click");
  assert.equal(currentView(), "home");
  showView("templates");
  assert.equal(getNode("tpl-payload").value, "ALPHA {{X}}");
  assert.match(getNode("tpl-preview").text, /ALPHA/);
  assert.doesNotMatch(getNode("tpl-preview").text, /edited/);
  assert.equal(getNode("tpl-dirty").hidden, true);
});

test("discarding a new template hides the form and later typing does not crash", async () => {
  await boot();
  getNode("tpl-new").fire("click");
  getNode("tpl-name").type("Brand new");
  showView("home");
  getNode("tpl-guard-discard").fire("click");
  showView("templates");
  assert.equal(form().hidden, true);
  assert.equal(getNode("tpl-empty").hidden, false);
  assert.doesNotThrow(() => getNode("tpl-payload").type("stray"));
  assert.doesNotThrow(() => getNode("tpl-name").type("stray"));
});

test("failed delete re-enables Save/Delete and a retry succeeds", async () => {
  await boot();
  open("Beta");
  ipc.handler = async () => {
    throw "locked file";
  };
  getNode("tpl-delete").fire("click");
  getNode("tpl-delete").fire("click"); // confirm
  await tick();
  assert.equal(getNode("tpl-save").disabled, false);
  assert.equal(getNode("tpl-delete").disabled, false);
  assert.match(getNode("tpl-banner-text").textContent, /locked file/);
  assert.equal(form().hidden, false);

  ipc.handler = async () => undefined;
  getNode("tpl-delete").fire("click");
  getNode("tpl-delete").fire("click");
  await tick();
  assert.equal(form().hidden, true);
  assert.equal(cardNamed("Beta"), undefined);
});
