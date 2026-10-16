// Integration tests for src/templates/templates.js driven through a minimal fake DOM and a
// mocked Tauri IPC. They exercise the real module: events, guards, rendering.
import test from "node:test";
import assert from "node:assert/strict";
import { readMarkup } from "./helpers/markup.js";

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
  connection_status: { get: () => "disconnected", set() {}, subscribe() {} },
};

// messages.js (imported by templates.js) initialises on import and needs the composer form.
getNode("message-form").message = getNode("message");
getNode("message-form").querySelector = () => getNode("message-submit");
for (const id of ["messages", "no-connection"]) getNode(id).toggleAttribute = () => {};

const SEED = [
  { id: "a", name: "Alpha", description: "", payload: "ALPHA {{X}}", variables: [{ name: "X", default: "1" }] },
  { id: "b", name: "Beta", description: "", payload: "BETA", variables: [] },
];

const { showView, currentView } = await import("../src/nav.js");

/** templates.js initialises on import, so it is imported once the fake backend is ready. */
let templatesModule;
const loadTemplatesModule = () => (templatesModule ??= import("../src/templates/templates.js"));

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
    await loadTemplatesModule();
  } else {
    getNode("tpl-banner-retry").fire("click"); // reload from the mocked IPC
    await tick();
  }
  // Return to a clean state: no guard, nothing selected.
  getNode("tpl-guard-cancel").fire("click");
}

const cards = () => getNode("tpl-list").children;
const cardNamed = (name) => cards().find((c) => c.text.includes(name));
const loadBtn = (card) => card.find((n) => n.classes.has("tload"))[0];
const open = (name) => loadBtn(cardNamed(name)).fire("click");
/** The delete ✕ only exists on the selected card. */
const delBtn = () => cards().find((c) => c.classes.has("on"))?.find((n) => n.classes.has("k-x"))[0];
const form = () => getNode("tpl-form");
const guard = () => getNode("tpl-guard");
const savedOk = () => ipc.calls.filter((c) => c.cmd === "save_templates");

test("deferred initial load: New/Save/shortcuts are refused and the persisted collection is untouched", async () => {
  const gate = deferred();
  ipc.calls = [];
  ipc.handler = (cmd) => (cmd === "load_templates" ? gate.promise : Promise.resolve());
  booted = true;
  const pending = loadTemplatesModule().catch(() => {});
  // The module initialises on import, which needs real (async) file loading first.
  for (let i = 0; i < 200 && !/Loading/.test(getNode("tpl-list").text); i++) await tick();

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
  await pending;
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
  delBtn().fire("click");
  delBtn().fire("click");
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
  delBtn().fire("click"); // discard the unsaved draft (new)
  delBtn().fire("click");
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
  delBtn().fire("click");
  delBtn().fire("click");
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
  open("Beta");
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
  assert.equal(delBtn().disabled, false);
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
  delBtn().fire("click");
  delBtn().fire("click"); // confirm
  await tick();
  assert.equal(getNode("tpl-save").disabled, false);
  assert.equal(delBtn().disabled, false);
  assert.match(getNode("tpl-banner-text").textContent, /locked file/);
  assert.equal(form().hidden, false);

  ipc.handler = async () => undefined;
  delBtn().fire("click");
  delBtn().fire("click");
  await tick();
  assert.equal(form().hidden, true);
  assert.equal(cardNamed("Beta"), undefined);
});

test("the saved-template store only follows confirmed load / save / delete results", async () => {
  const { getSavedTemplates } = await import("../src/templates/template-store.js");
  await boot();
  assert.deepEqual(getSavedTemplates().map((t) => t.id), ["a", "b"], "published after load");

  open("Beta");
  getNode("tpl-name").type("Beta edited");
  assert.equal(getSavedTemplates().find((t) => t.id === "b").name, "Beta", "drafts are never published");

  ipc.handler = async () => {
    throw "refused: would break the auto reply rule";
  };
  getNode("tpl-save").fire("click");
  await tick();
  assert.equal(getSavedTemplates().find((t) => t.id === "b").name, "Beta", "failed save publishes nothing");
  assert.match(getNode("tpl-banner-text").textContent, /would break the auto reply rule/);

  ipc.handler = async () => undefined;
  getNode("tpl-save").fire("click");
  await tick();
  assert.equal(getSavedTemplates().find((t) => t.id === "b").name, "Beta edited", "published after save");

  delBtn().fire("click");
  delBtn().fire("click");
  await tick();
  assert.deepEqual(getSavedTemplates().map((t) => t.id), ["a"], "published after delete");

  ipc.handler = async () => {
    throw "boom";
  };
  getNode("tpl-banner-retry").fire("click");
  await tick();
  assert.equal(getSavedTemplates(), null, "unknown library after a failed load");
});

// Ã¢â€â‚¬Ã¢â€â‚¬ Autobuild option Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

globalThis.localStorage ??= { getItem: () => null, setItem() {} };
const abCalls = () => ipc.calls.filter((c) => c.cmd === "auto_build_message_cmd");
const sends = () => ipc.calls.filter((c) => c.cmd === "send_message");
const check = (id, on) => {
  const box = getNode(id);
  box.checked = on;
  box.fire("change");
};

/** Handler: templates load as usual, Autobuild answers through `ab(args)`. */
function useBackend(stored, ab) {
  ipc.handler = async (cmd, args) => {
    if (cmd === "load_templates") return { templates: structuredClone(stored), seeded: false };
    if (cmd === "auto_build_message_cmd") return ab(args);
    return undefined;
  };
}

/** Reloads the collection and opens `name` from a clean state (no stale draft from a previous test). */
async function openFresh(name) {
  await boot();
  showView("templates", { force: true });
  if (!form().hidden) {
    showView("home");
    if (!guard().hidden) getNode("tpl-guard-discard").fire("click");
  }
  open(name === "Alpha" ? "Beta" : "Alpha");
  open(name);
  showView("templates", { force: true });
}

async function twoSessions() {
  const { sessions, store } = await import("../src/session/sessions.js");
  const ids = [];
  for (const port of [9001, 9002]) {
    const { id, done } = sessions.start({ mode: "client", label: `h:${port}`, req: { type: "ClientConnectRequest", ip: "h", port } });
    await done;
    sessions.handleStatus({ session_id: id, attempt: store.get(id).attempt, status: "connected" });
    ids.push(id);
  }
  return { store, ids };
}

test("autobuild flags: dirty tracking, save payload, badge and legacy defaults", async () => {
  await openFresh("Alpha");
  assert.equal(getNode("tpl-autobuild").checked, false, "legacy template loads with the option off");
  assert.equal(getNode("tpl-autobuild-noetb").checked, false);
  assert.equal(getNode("tpl-autobuild-noetb").disabled, true, "No ETB needs Autobuild");
  assert.equal(getNode("tpl-dirty").hidden, true);
  assert.equal(cardNamed("Alpha").text.includes("Autobuild"), false);

  useBackend(SEED, async () => ({ output: "B" }));
  check("tpl-autobuild", true);
  assert.equal(getNode("tpl-dirty").hidden, false, "toggling Autobuild makes the draft dirty");
  assert.equal(getNode("tpl-autobuild-noetb").disabled, false);
  assert.equal(cardNamed("Alpha").text.includes("Autobuild"), false, "no Autobuild badge on the card");
  check("tpl-autobuild", false);
  assert.equal(getNode("tpl-dirty").hidden, true, "back to the saved value: clean");
  check("tpl-autobuild", true);
  check("tpl-autobuild-noetb", true);

  getNode("tpl-save").fire("click");
  await tick();
  const alpha = savedOk().at(-1).args.templates.find((t) => t.id === "a");
  assert.equal(alpha.autobuild, true);
  assert.equal(alpha.autobuild_no_etb, true);
  assert.equal(getNode("tpl-dirty").hidden, true);

  const { getSavedTemplates } = await import("../src/templates/template-store.js");
  assert.equal(getSavedTemplates().find((t) => t.id === "a").autobuild, true);
  assert.equal(getSavedTemplates().find((t) => t.id === "b").autobuild_no_etb, false);

  // New templates default to off.
  getNode("tpl-new").fire("click");
  assert.equal(getNode("tpl-autobuild").checked, false);
  assert.equal(getNode("tpl-autobuild-noetb").checked, false);
  delBtn().fire("click");
  delBtn().fire("click");
});

test("Send builds first and keeps the target captured before the await; double click sends once", async () => {
  const { store, ids } = await twoSessions();
  const [s1, s2] = ids;
  store.select(s1);
  await openFresh("Alpha");
  const gate = deferred();
  useBackend(SEED, () => gate.promise);
  check("tpl-autobuild", true);
  check("tpl-autobuild-noetb", true);
  ipc.calls = [];
  getNode("tpl-send").fire("click");
  getNode("tpl-send").fire("click"); // double click while building
  getNode("tpl-load").fire("click"); // also guarded
  assert.equal(getNode("tpl-send").disabled, true);
  store.select(s2); // the user switches session while the build is pending
  gate.resolve({ output: "<VT>BUILT<FS><CR>" });
  await tick();
  await tick();
  assert.equal(sends().length, 1, "one send only");
  assert.equal(sends()[0].args.sessionId, s1, "captured target, not the new selection");
  assert.equal(sends()[0].args.payload.message, "<VT>BUILT<FS><CR>");
  const sendBuild = abCalls().find((c) => c.args.req.input === "ALPHA 1");
  assert.deepEqual(sendBuild.args, { req: { input: "ALPHA 1", no_etb: true } });
  assert.equal(store.get(s2).draft ?? "", "", "nothing leaked to the other session");
  assert.equal(getNode("tpl-send").disabled, false, "re-enabled after the build");
  assert.equal(getNode("tpl-banner").hidden, true);

  // Option off: the resolved text goes out untouched and nothing is built.
  check("tpl-autobuild", false);
  ipc.calls = [];
  store.select(s1);
  getNode("tpl-send").fire("click");
  await tick();
  assert.equal(abCalls().length, 0);
  assert.equal(sends().at(-1).args.payload.message, "ALPHA 1");
  store.select(s1);
});

test("Load in composer delivers the built text to the session captured before the await", async () => {
  const { store, ids } = await twoSessions();
  const [s1, s2] = ids;
  store.select(s1);
  await openFresh("Alpha");
  const gate = deferred();
  useBackend(SEED, () => gate.promise);
  check("tpl-autobuild", true);
  getNode("tpl-load").fire("click");
  store.select(s2);
  gate.resolve({ output: "LOADED" });
  await tick();
  await tick();
  assert.equal(store.get(s1).draft, "LOADED");
  assert.notEqual(store.get(s2).draft, "LOADED");
  assert.deepEqual(abCalls().at(-1).args, { req: { input: "ALPHA 1", no_etb: false } });

  // The captured session is closed during the build: nothing is loaded elsewhere.
  store.select(s1);
  const gate2 = deferred();
  useBackend(SEED, () => gate2.promise);
  getNode("tpl-load").fire("click");
  store.remove?.(s1);
  store.select(s2);
  gate2.resolve({ output: "ORPHAN" });
  await tick();
  await tick();
  assert.notEqual(store.get(s2).draft, "ORPHAN");
});

test("a failed build shows the error and neither sends nor loads", async () => {
  const { store, ids } = await twoSessions();
  store.select(ids[0]);
  await openFresh("Alpha");
  useBackend(SEED, async () => {
    throw "build exploded";
  });
  check("tpl-autobuild", true);
  await tick();
  assert.match(getNode("tpl-preview").text, /Autobuild failed: build exploded/);
  assert.doesNotMatch(getNode("tpl-preview").text, /ALPHA 1/, "error replaces the text, no stale text");
  assert.match(getNode("tpl-validity").text, /Ready/, "existing validity message is kept");
  ipc.calls = [];
  const before = store.get(ids[0]).draft;
  getNode("tpl-send").fire("click");
  await tick();
  assert.equal(sends().length, 0);
  assert.match(getNode("tpl-banner-text").textContent, /Autobuild failed: build exploded.*Nothing was sent/);
  assert.equal(getNode("tpl-send").disabled, false, "guard released after the failure");
  getNode("tpl-load").fire("click");
  await tick();
  assert.equal(store.get(ids[0]).draft, before, "nothing loaded");
  assert.match(getNode("tpl-banner-text").textContent, /Nothing was loaded/);
});

test("single preview box: plain when off, built when on, no late overwrite", async () => {
  await openFresh("Alpha");
  const gates = [];
  useBackend(SEED, (args) => {
    const gate = deferred();
    gates.push({ gate, input: args.req.input });
    return gate.promise;
  });
  const html = readMarkup();
  assert.equal((html.match(/id="tpl-preview"/g) ?? []).length, 1, "exactly one preview element");
  assert.doesNotMatch(html, /tpl-built/, "no second preview box");
  assert.doesNotMatch(html, /Preview Ã¢â‚¬â€ after Autobuild/);

  ipc.calls = [];
  assert.match(getNode("tpl-preview").text, /ALPHA 1/);
  assert.doesNotMatch(html, /tpl-preview-tag/, "no After Autobuild pill");
  assert.equal(abCalls().length, 0, "off: no invoke");
  check("tpl-autobuild", true);
  assert.equal(getNode("tpl-preview").classes.has("pending"), true);
  assert.match(getNode("tpl-preview").text, /ALPHA 1/, "plain text while pending");
  getNode("tpl-payload").type("ALPHA second");
  assert.equal(gates.length, 2);
  gates[1].gate.resolve({ output: "NEW RESULT" });
  await tick();
  assert.equal(getNode("tpl-preview").classes.has("pending"), false);
  gates[0].gate.resolve({ output: "OLD RESULT" });
  await tick();
  assert.match(getNode("tpl-preview").text, /NEW RESULT/);
  assert.doesNotMatch(getNode("tpl-preview").text, /OLD/);

  // Toggling off shows the plain text again; a late result must not overwrite it.
  getNode("tpl-payload").type("ALPHA third");
  const pending = gates.at(-1);
  check("tpl-autobuild", false);
  assert.match(getNode("tpl-preview").text, /ALPHA third/);
  pending.gate.resolve({ output: "LATE BUILT" });
  await tick();
  assert.match(getNode("tpl-preview").text, /ALPHA third/);
  assert.doesNotMatch(getNode("tpl-preview").text, /LATE BUILT/);

  // Switching to another template drops a result still in flight for the previous one.
  check("tpl-autobuild", true);
  const inflight = gates.at(-1);
  showView("templates");
  getNode("tpl-guard-discard").fire("click");
  open("Beta");
  inflight.gate.resolve({ output: "FROM ALPHA" });
  await tick();
  assert.doesNotMatch(getNode("tpl-preview").text, /FROM ALPHA/);
});
test("late Load result does not navigate away from another view nor bypass its leave guard", async () => {
  const { setLeaveGuard } = await import("../src/nav.js");
  const { store, ids } = await twoSessions();
  const [s1, s2] = ids;
  store.select(s1);
  await openFresh("Alpha");
  const gate = deferred();
  useBackend(SEED, () => gate.promise);
  check("tpl-autobuild", true);
  getNode("tpl-load").fire("click");
  // The user leaves Templates (its own guard satisfied) for a view with unsaved edits.
  let guardCalls = 0;
  setLeaveGuard("autoreply", () => {
    guardCalls += 1;
    return false;
  });
  showView("autoreply", { force: true });
  store.select(s2);
  gate.resolve({ output: "LATE" });
  await tick();
  await tick();
  assert.equal(currentView(), "autoreply", "no forced navigation");
  assert.equal(guardCalls, 0, "guard neither consulted nor bypassed");
  assert.equal(store.get(s1).draft, "LATE", "draft still loaded into the captured session");
  assert.notEqual(store.get(s2).draft, "LATE");
  assert.equal(getNode("tpl-load").disabled, false, "pending flags reset");
  setLeaveGuard("autoreply", () => true);

  // Normal path: still in Templates, still the same draft => navigates to the session view.
  showView("templates", { force: true });
  store.select(s1);
  const gate2 = deferred();
  useBackend(SEED, () => gate2.promise);
  getNode("tpl-load").fire("click");
  gate2.resolve({ output: "NORMAL" });
  await tick();
  await tick();
  assert.equal(store.get(s1).draft, "NORMAL");
  assert.equal(currentView(), "session");
});


/** Types into a variable input (its `type` property is the HTML type, not the helper). */
const typeVar = (node, value) => {
  node.value = value;
  node.fire("input");
};

test("save, delete and send show no confirmation banner and clear a previous error", async () => {
  await boot();
  useBackend(SEED, () => ({ output: "x" }));
  open("Alpha");
  getNode("tpl-payload").type("ALPHA z");
  getNode("tpl-banner").className = "banner error";
  getNode("tpl-save").fire("click");
  await tick();
  assert.equal(savedOk().length, 1);
  assert.equal(getNode("tpl-banner").hidden, true, "save: no banner");

  getNode("tpl-banner").className = "banner error";
  delBtn().fire("click");
  delBtn().fire("click");
  await tick();
  assert.equal(getNode("tpl-banner").hidden, true, "delete: no banner");
});

test("changing a variable value persists it without dirtying, guarding or locking the editor", async () => {
  await boot();
  open("Alpha");
  const input = () => getNode("tpl-vars").find((n) => n.tag === "input")[0];
  ipc.calls = [];
  typeVar(input(), "42");
  assert.equal(getNode("tpl-dirty").hidden, true);
  assert.equal(input().readOnly, false, "editor not locked");
  await tick();
  assert.equal(savedOk().length, 1);
  const written = savedOk()[0].args.templates;
  assert.deepEqual(written.find((t) => t.id === "a").variables, [{ name: "X", default: "42" }]);
  assert.equal(written.find((t) => t.id === "a").payload, "ALPHA {{X}}");
  assert.equal(getNode("tpl-banner").hidden, true);

  // Switching templates is not guarded and reopening keeps the new value.
  open("Beta");
  assert.equal(guard().hidden, true);
  open("Alpha");
  assert.equal(input().value, "42");
});

test("beforeunload is prevented while a background variable write is in flight", async () => {
  await boot();
  open("Alpha");
  const gate = deferred();
  ipc.handler = async (cmd) => (cmd === "save_templates" ? gate.promise : undefined);
  const unload = () => {
    const ev = { prevented: false, returnValue: undefined, preventDefault() { this.prevented = true; } };
    for (const fn of winListeners.beforeunload ?? []) fn(ev);
    return ev.prevented;
  };
  assert.equal(unload(), false, "idle: unloading allowed");
  typeVar(getNode("tpl-vars").find((n) => n.tag === "input")[0], "7");
  assert.equal(unload(), true, "write in flight: unloading prevented");
  gate.resolve(undefined);
  await tick();
  await tick();
  assert.equal(unload(), false, "settled: unloading allowed again");
});

test("variable writes never persist unsaved template edits; discard keeps the value", async () => {
  await boot();
  open("Alpha");
  getNode("tpl-name").type("Renamed");
  getNode("tpl-payload").type("ALPHA {{X}} more");
  getNode("tpl-autobuild").checked = true;
  getNode("tpl-autobuild").fire("change");
  ipc.calls = [];
  typeVar(getNode("tpl-vars").find((n) => n.tag === "input")[0], "7");
  await tick();
  const a = savedOk().at(-1).args.templates.find((t) => t.id === "a");
  assert.equal(a.name, "Alpha");
  assert.equal(a.payload, "ALPHA {{X}}");
  assert.equal(Boolean(a.autobuild), false);
  assert.deepEqual(a.variables, [{ name: "X", default: "7" }]);
  assert.equal(getNode("tpl-dirty").hidden, false, "template edits still dirty");

  showView("templates", { force: true });
  showView("home");
  assert.equal(guard().hidden, false);
  getNode("tpl-guard-discard").fire("click");
  await tick();
  showView("templates", { force: true });
  assert.equal(getNode("tpl-name").value, "Alpha");
  assert.equal(getNode("tpl-payload").value, "ALPHA {{X}}");
  assert.equal(getNode("tpl-autobuild").checked, false);
  assert.equal(getNode("tpl-vars").find((n) => n.tag === "input")[0].value, "7");
});

test("a rejected variable write shows an error and keeps the typed value", async () => {
  await boot();
  open("Alpha");
  ipc.handler = async (cmd) => {
    if (cmd === "save_templates") throw "used by a rule";
    return undefined;
  };
  const input = () => getNode("tpl-vars").find((n) => n.tag === "input")[0];
  typeVar(input(), "");
  await tick();
  assert.equal(getNode("tpl-banner").hidden, false);
  assert.match(getNode("tpl-banner-text").textContent, /Could not save: used by a rule/);
  assert.equal(input().value, "");
  assert.equal(input().readOnly, false);
});

const XY = [{ id: "xy", name: "Pair", description: "", payload: "{{X}} {{Y}}", variables: [{ name: "X", default: "1" }, { name: "Y", default: "2" }] }];

test("an unsaved payload edit that drops a placeholder never erases its saved value", async () => {
  await boot(XY);
  open("Pair");
  getNode("tpl-payload").type("{{Y}}");
  ipc.calls = [];
  const inputs = () => getNode("tpl-vars").find((n) => n.tag === "input");
  assert.equal(inputs().length, 1);
  typeVar(inputs()[0], "9");
  await tick();
  const written = savedOk().at(-1).args.templates.find((t) => t.id === "xy");
  assert.equal(written.payload, "{{X}} {{Y}}");
  assert.deepEqual(written.variables, [{ name: "X", default: "1" }, { name: "Y", default: "9" }]);

  // Re-typing the placeholder restores its value; discarding keeps both values.
  getNode("tpl-payload").type("{{X}} {{Y}}");
  assert.deepEqual(inputs().map((n) => n.value), ["1", "9"]);
  getNode("tpl-payload").type("{{Y}}");
  showView("templates", { force: true });
  showView("home");
  getNode("tpl-guard-discard").fire("click");
  await tick();
  showView("templates", { force: true });
  assert.equal(getNode("tpl-payload").value, "{{X}} {{Y}}");
  assert.deepEqual(inputs().map((n) => n.value), ["1", "9"]);
});

test("a rejected variable write does not drop a newer queued value", async () => {
  await boot();
  open("Alpha");
  const first = deferred();
  let n = 0;
  ipc.handler = async (cmd) => {
    if (cmd !== "save_templates") return undefined;
    if (++n === 1) return first.promise;
    return undefined;
  };
  ipc.calls = [];
  const input = () => getNode("tpl-vars").find((x) => x.tag === "input")[0];
  typeVar(input(), "");
  typeVar(input(), "5");
  first.reject("empty value");
  await tick();
  await tick();
  const calls = savedOk();
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[1].args.templates.find((t) => t.id === "a").variables, [{ name: "X", default: "5" }]);
  assert.equal(getNode("tpl-banner").hidden, true);
});

test("removing and restoring a placeholder before any variable edit keeps its saved default", async () => {
  await boot(XY);
  open("Pair");
  const inputs = () => getNode("tpl-vars").find((n) => n.tag === "input");
  getNode("tpl-payload").type("{{Y}}");
  getNode("tpl-payload").type("{{X}} {{Y}}");
  assert.deepEqual(inputs().map((n) => n.value), ["1", "2"]);
  ipc.calls = [];
  typeVar(inputs()[1], "9");
  await tick();
  const written = savedOk().at(-1).args.templates.find((t) => t.id === "xy");
  assert.deepEqual(written.variables, [{ name: "X", default: "1" }, { name: "Y", default: "9" }]);
});

test("a failed delete resumes the variable write queued behind a background write", async () => {
  await boot();
  open("Alpha");
  const first = deferred();
  let n = 0;
  ipc.handler = async (cmd) => {
    if (cmd !== "save_templates") return undefined;
    n++;
    if (n === 1) return first.promise;
    if (n === 2) throw "delete refused";
    return undefined;
  };
  ipc.calls = [];
  const input = () => getNode("tpl-vars").find((x) => x.tag === "input")[0];
  typeVar(input(), "2");
  typeVar(input(), "3");
  open("Beta");
  delBtn().fire("click");
  delBtn().fire("click");
  first.resolve();
  await tick();
  await tick();
  await tick();
  const calls = savedOk();
  assert.equal(calls.length, 3);
  assert.equal(calls[0].args.templates.find((t) => t.id === "a").variables[0].default, "2");
  assert.equal(calls[1].args.templates.some((t) => t.id === "b"), false, "second call is the failed delete");
  assert.deepEqual(calls[2].args.templates.find((t) => t.id === "a").variables, [{ name: "X", default: "3" }]);
  assert.equal(calls[2].args.templates.some((t) => t.id === "b"), true);
});

const crosses = () => cards().flatMap((c) => c.find((n) => n.classes.has("k-x")));

test("the delete ✕ exists only on the selected card and needs two clicks", async () => {
  await boot();
  open("Alpha");
  assert.equal(crosses().length, 1);
  assert.equal(cardNamed("Alpha").find((n) => n.classes.has("k-x")).length, 1);
  assert.equal(delBtn().textContent, "✕");

  delBtn().fire("click");
  assert.equal(delBtn().textContent, "Sure?");
  assert.equal(delBtn().classes.has("armed"), true);
  assert.equal(savedOk().length, 0, "the first click only arms");

  delBtn().fire("click");
  await tick();
  assert.equal(cardNamed("Alpha"), undefined);
  assert.equal(form().hidden, true);
});

test("selecting another card moves the ✕ and disarms it", async () => {
  await boot();
  open("Alpha");
  delBtn().fire("click");
  assert.equal(delBtn().classes.has("armed"), true);
  open("Beta");
  assert.equal(crosses().length, 1);
  assert.equal(cardNamed("Beta").find((n) => n.classes.has("k-x")).length, 1);
  assert.equal(delBtn().textContent, "✕");
  assert.equal(delBtn().classes.has("armed"), false);
  delBtn().fire("click");
  delBtn().fire("click");
  await tick();
});

test("the save pill shows only while the draft has unsaved changes", async () => {
  await boot();
  open("Alpha");
  assert.equal(getNode("tpl-dirty").hidden, true);
  assert.equal(form().classes.has("pending"), false);

  getNode("tpl-payload").type("ALPHA edited");
  assert.equal(getNode("tpl-dirty").hidden, false);
  assert.equal(form().classes.has("pending"), true, "room is reserved under the pill");

  getNode("tpl-save").fire("click");
  await tick();
  assert.equal(getNode("tpl-dirty").hidden, true);
  assert.equal(form().classes.has("pending"), false);
});

test("a new unsaved template has no 'unsaved' badge on its card", async () => {
  await boot();
  getNode("tpl-new").fire("click");
  await tick();
  assert.equal(
    getNode("tpl-list").find((n) => n.classes?.has("badge") && n.textContent === "unsaved").length,
    0,
  );
});

test("a search that excludes the open template still lists it, so it stays deletable", async () => {
  await boot();
  open("Beta");
  getNode("tpl-search").type("alpha");
  assert.equal(cardNamed("Alpha") !== undefined, true);
  assert.equal(cardNamed("Beta") !== undefined, true, "the open template stays listed");
  assert.equal(delBtn() !== undefined, true);

  getNode("tpl-search").type("");
  delBtn().fire("click");
  delBtn().fire("click");
  await tick();
});
