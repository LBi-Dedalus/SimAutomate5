// Integration tests for src/autobuild-view.js with a minimal fake DOM and injected backend/storage.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { initAutobuildView } from "../src/autobuild-view.js";
import { RECENT_BUILDS_KEY } from "../src/autobuild-recent-core.js";

class FakeNode {
  constructor(tag = "div", id = "") {
    this.tag = tag;
    this.id = id;
    this.children = [];
    this.listeners = {};
    this.classes = new Set();
    this.dataset = {};
    this.attrs = {};
    this.value = "";
    this.checked = false;
    this.disabled = false;
    this.textContent = "";
    const self = this;
    this.classList = {
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
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  fire(type) {
    for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {}, target: this });
  }
  appendChild(c) {
    this.children.push(c);
    return c;
  }
  replaceChildren() {
    this.children = [];
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
  focus() {
    this.doc.activeElement = this;
  }
  find(cls) {
    return this.children.find((c) => c.classes.has(cls));
  }
  get text() {
    return this.textContent + this.children.map((c) => c.text).join(" ");
  }
}

const tick = () => new Promise((r) => setTimeout(r, 0));

function setup({ stored, active = "s1", sessions = ["s1"], invoke, storage } = {}) {
  const nodes = new Map();
  const doc = {
    activeElement: null,
    getElementById(id) {
      if (!nodes.has(id)) nodes.set(id, Object.assign(new FakeNode("div", id), { doc }));
      return nodes.get(id);
    },
    createElement: (tag) => Object.assign(new FakeNode(tag), { doc }),
  };
  const data = new Map(stored ? [[RECENT_BUILDS_KEY, stored]] : []);
  const store = storage ?? {
    getItem: (k) => (data.has(k) ? data.get(k) : null),
    setItem: (k, v) => data.set(k, v),
    removeItem: (k) => data.delete(k),
  };
  const log = { errors: [], shown: [], calls: [], drafts: {} };
  let clock = 1000;
  const view = initAutobuildView({
    doc,
    storage: store,
    invoke:
      invoke ??
      (async (cmd, args) => {
        log.calls.push({ cmd, args });
        return { output: `built:${args.req.input}:${args.req.no_etb}` };
      }),
    activeId: () => active,
    hasSession: (id) => sessions.includes(id),
    setDraft: (id, text) => {
      log.drafts[id] = text;
      return true;
    },
    showSession: (t) => log.shown.push(t),
    logError: (m, w) => log.errors.push([m, w]),
    now: () => (clock += 1000),
  });
  const $ = (id) => doc.getElementById(id);
  return {
    $,
    doc,
    log,
    data,
    view,
    cards: () => $("ab-list").children,
    type(input, noEtb = false) {
      $("ab-input").value = input;
      $("ab-no-etb").checked = noEtb;
    },
  };
}

test("renders an empty state when there is no recent autobuild", () => {
  const t = setup();
  assert.equal(t.cards().length, 0);
  assert.equal(t.$("ab-empty").classes.has("hidden"), false);
  assert.equal(t.$("ab-list").classes.has("hidden"), true);
  assert.equal(t.$("ab-clear").disabled, true);
});

test("a successful Build records a card; a failed build or empty input does not", async () => {
  const t = setup();
  t.type("H|a");
  t.$("build-message").fire("click");
  await tick();
  assert.equal(t.$("ab-output").value, "built:H|a:false");
  assert.equal(t.cards().length, 1);
  const card = t.cards()[0];
  assert.equal(card.find("ab-load").find("ab-top").find("ab-kind").textContent, "ASTM");
  assert.match(card.text, /H\|a/);
  assert.equal(t.$("ab-clear").disabled, false);
  assert.equal(t.$("ab-count-label").textContent, "(1)");
  assert.equal(t.log.shown.length, 0, "Build alone does not navigate");

  // failure
  const f = setup({
    invoke: async () => {
      throw new Error("boom");
    },
  });
  f.type("MSH|x");
  f.$("build-message").fire("click");
  await tick();
  assert.equal(f.cards().length, 0);
  assert.equal(f.data.has(RECENT_BUILDS_KEY), false);
  assert.match(f.$("autobuild-error").textContent, /boom/);
  assert.equal(f.log.errors.length, 1);

  // empty
  const e = setup();
  e.type("   ");
  e.$("build-message").fire("click");
  await tick();
  assert.equal(e.log.calls.length, 0);
  assert.equal(e.cards().length, 0);
  assert.match(e.$("autobuild-error").textContent, /Enter a message/);
});

test("rebuilding the same input keeps a single card on top", async () => {
  const t = setup();
  t.type("A");
  t.$("build-message").fire("click");
  await tick();
  t.type("B");
  t.$("build-message").fire("click");
  await tick();
  t.type("A");
  t.$("build-message").fire("click");
  await tick();
  assert.equal(t.cards().length, 2);
  assert.match(t.cards()[0].text, /A/);
});

test("Build and copy delivers to the captured session, records, and shows the session", async () => {
  const t = setup();
  t.type("MSH|x", true);
  t.$("ab-form").fire("submit");
  await tick();
  assert.deepEqual(t.log.drafts, { s1: "built:MSH|x:true" });
  assert.deepEqual(t.log.shown, ["s1"]);
  assert.equal(t.cards().length, 1);
  assert.ok(t.cards()[0].find("ab-load").find("ab-top").find("ab-noetb"));
});

test("Build and copy without a session reports the error but still records the successful build", async () => {
  const t = setup({ active: null });
  t.type("A");
  t.$("ab-form").fire("submit");
  await tick();
  assert.match(t.$("autobuild-error").textContent, /No session is open/);
  assert.deepEqual(t.log.shown, []);
  assert.equal(t.$("ab-output").value, "built:A:false");
  assert.equal(t.cards().length, 1);
});

const STORED = JSON.stringify([
  { id: "1", input: "H|one\rP|1", output: "OUT1", noEtb: true, lastUsed: 5 },
  { id: "2", input: "<b>x</b>", output: "OUT2", noEtb: false, lastUsed: 4 },
]);

test("loads stored entries, renders text via textContent and loads a card without building", () => {
  const t = setup({ stored: STORED });
  assert.equal(t.cards().length, 2);
  assert.equal(t.cards()[1].find("ab-load").children.at(-1).textContent, "<b>x</b>");
  t.cards()[0].find("ab-load").fire("click");
  assert.equal(t.$("ab-input").value, "H|one\rP|1");
  assert.equal(t.$("ab-output").value, "OUT1");
  assert.equal(t.$("ab-no-etb").checked, true);
  assert.equal(t.log.calls.length, 0, "loading never builds");
  assert.equal(t.log.shown.length, 0);
  assert.ok(t.cards()[0].classes.has("on"));
  assert.equal(t.cards()[0].find("ab-load").attrs["aria-pressed"], "true");
  assert.equal(t.cards()[1].classes.has("on"), false);
});

test("Use in session copies the stored output, no rebuild, and shows the session", () => {
  const t = setup({ stored: STORED });
  t.cards()[1].find("ab-use").fire("click");
  assert.deepEqual(t.log.drafts, { s1: "OUT2" });
  assert.deepEqual(t.log.shown, ["s1"]);
  assert.equal(t.log.calls.length, 0);
});

test("Use in session reports no session and closed session", () => {
  const none = setup({ stored: STORED, active: null });
  none.cards()[0].find("ab-use").fire("click");
  assert.match(none.$("autobuild-error").textContent, /No session is open/);
  assert.deepEqual(none.log.shown, []);

  const closed = setup({ stored: STORED, active: "gone", sessions: ["s1"] });
  closed.cards()[0].find("ab-use").fire("click");
  assert.match(closed.$("autobuild-error").textContent, /closed/);
  assert.deepEqual(closed.log.drafts, {});
});

test("delete removes one entry; clear removes all", () => {
  const t = setup({ stored: STORED });
  t.cards()[0].find("ab-del").fire("click");
  assert.equal(t.cards().length, 1);
  assert.deepEqual(JSON.parse(t.data.get(RECENT_BUILDS_KEY)).map((e) => e.id), ["2"]);
  t.$("ab-clear").fire("click");
  assert.equal(t.cards().length, 0);
  assert.equal(t.data.has(RECENT_BUILDS_KEY), false);
  assert.equal(t.$("ab-empty").classes.has("hidden"), false);
  assert.equal(t.$("ab-clear").disabled, true);
});

test("corrupt stored data is shown non-blockingly and logged", () => {
  const t = setup({ stored: "{bad" });
  assert.equal(t.cards().length, 0);
  assert.equal(t.$("ab-note").classes.has("hidden"), false);
  assert.match(t.$("ab-note").textContent, /unreadable/);
  assert.equal(t.log.errors.length, 1);
});

test("a storage that always fails does not break building", async () => {
  const storage = {
    getItem: () => null,
    setItem() {
      throw new Error("quota");
    },
    removeItem() {},
  };
  const t = setup({ storage });
  t.type("A");
  t.$("build-message").fire("click");
  await tick();
  assert.equal(t.$("ab-output").value, "built:A:false");
  assert.match(t.$("ab-note").textContent, /Cannot save/);
});

test("a build finishing after another card was loaded does not overwrite the builder output", async () => {
  let release;
  const t = setup({
    stored: STORED,
    invoke: () => new Promise((r) => (release = () => r({ output: "LATE" }))),
  });
  t.type("slow");
  t.$("ab-form").fire("submit");
  t.cards()[1].find("ab-load").fire("click");
  release();
  await tick();
  assert.equal(t.$("ab-input").value, "<b>x</b>");
  assert.equal(t.$("ab-output").value, "OUT2", "stale output not applied");
  assert.deepEqual(t.log.drafts, { s1: "LATE" }, "delivery still happens");
  assert.ok(t.view.entries().some((e) => e.input === "slow" && e.output === "LATE"), "still recorded");
});

test("a build finishing after the input was edited does not overwrite the output", async () => {
  let release;
  const t = setup({ invoke: () => new Promise((r) => (release = () => r({ output: "LATE" }))) });
  t.type("a");
  t.$("build-message").fire("click");
  t.type("ab");
  t.$("ab-input").fire("input");
  release();
  await tick();
  assert.equal(t.$("ab-output").value, "");
  assert.equal(t.cards().length, 1);
});

test("sync() after hydration marks the matching card as pressed", () => {
  const t = setup({ stored: STORED });
  assert.equal(t.cards()[0].find("ab-load").attrs["aria-pressed"], "false");
  // config.js hydrates fields without firing events.
  t.type("H|one\rP|1", true);
  t.view.sync();
  assert.equal(t.cards()[0].find("ab-load").attrs["aria-pressed"], "true");
  assert.ok(t.cards()[0].classes.has("on"));
  assert.equal(t.cards()[1].find("ab-load").attrs["aria-pressed"], "false");
});

test("loading a card keeps the same controls and focus", () => {
  const t = setup({ stored: STORED });
  const before = t.cards()[1];
  const btn = before.find("ab-load");
  btn.focus();
  btn.fire("click");
  assert.equal(t.cards()[1], before);
  assert.equal(t.doc.activeElement, btn);
  assert.equal(btn.attrs["aria-pressed"], "true");
  assert.ok(before.classes.has("on"));
  assert.equal(t.cards()[0].classes.has("on"), false);
});

test("delete moves focus to the next card, else previous, else the input; clear focuses the input", () => {
  const t = setup({ stored: STORED });
  t.cards()[0].find("ab-del").fire("click");
  assert.equal(t.doc.activeElement, t.cards()[0].find("ab-load"));
  const three = setup({
    stored: JSON.stringify([
      { id: "1", input: "A", lastUsed: 3 },
      { id: "2", input: "B", lastUsed: 2 },
    ]),
  });
  three.cards()[1].find("ab-del").fire("click");
  assert.equal(three.doc.activeElement, three.cards()[0].find("ab-load"));
  three.cards()[0].find("ab-del").fire("click");
  assert.equal(three.doc.activeElement, three.$("ab-input"));

  const c = setup({ stored: STORED });
  c.$("ab-clear").fire("click");
  assert.equal(c.doc.activeElement, c.$("ab-input"));
});

test("markup: dedicated nav view, no dialog, composer navigates to the view", () => {
  const html = readFileSync(new URL("../src/index.html", import.meta.url), "utf8");
  assert.ok(html.includes('data-view="autobuild"'));
  assert.ok(html.includes('id="view-autobuild"'));
  assert.ok(html.includes('data-view-panel="autobuild"'));
  assert.ok(html.includes('data-nav="autobuild"'));
  assert.equal(html.includes('<dialog id="autobuild"'), false);
  assert.equal(html.includes('commandfor="autobuild"'), false);
  for (const id of ["ab-input", "ab-output", "ab-no-etb", "ab-list", "ab-clear", "ab-empty", "ab-note", "build-message", "autobuild-error"]) {
    assert.ok(html.includes(`id="${id}"`), id);
  }
  for (const name of ["input", "output", "no-etb"]) {
    assert.match(html, new RegExp(`name="${name}"[^>]*data-persist|data-persist[^>]*name="${name}"`, "s"));
  }
});
