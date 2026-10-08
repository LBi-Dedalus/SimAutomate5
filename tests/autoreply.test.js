// Integration tests for src/autoreply.js driven through a minimal fake DOM and a mocked
// Tauri IPC (the real module: events, guards, rendering, persistence calls).
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
    this.attrs = {};
    this.value = "";
    this.checked = false;
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
  fire(type) {
    for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {}, stopPropagation() {}, target: this });
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
  focus() {}
  select() {}
  /** Simulates typing into a text field. */
  type(value) {
    this.value = value;
    this.fire("input");
  }
  /** Simulates choosing a value in a select / toggling a checkbox. */
  choose(value) {
    if (typeof value === "boolean") this.checked = value;
    else this.value = value;
    this.fire("change");
  }
  get text() {
    return this.textContent + this.children.map((c) => c.text ?? c.textContent ?? "").join("");
  }
}

const nodes = new Map();
const winListeners = {};
const getNode = (id) => {
  if (!nodes.has(id)) nodes.set(id, new FakeNode("div", id));
  return nodes.get(id);
};
const store = new Map();
globalThis.document = {
  getElementById: getNode,
  createElement: (tag) => new FakeNode(tag),
  createTextNode: (t) => ({ textContent: t }),
  querySelectorAll: () => [],
  addEventListener() {},
};
const ipc = { handler: async () => ({}), calls: [] };
globalThis.window = {
  addEventListener: (type, fn) => (winListeners[type] ??= []).push(fn),
  localStorage: {
    getItem: (k) => store.get(k) ?? null,
    setItem: (k, v) => store.set(k, v),
  },
  __TAURI__: {
    core: {
      invoke: (cmd, args) => {
        ipc.calls.push({ cmd, args });
        return ipc.handler(cmd, args);
      },
    },
  },
};

const { showView, currentView } = await import("../src/nav.js");
const { publishSavedTemplates, clearSavedTemplates } = await import("../src/template-store.js");
await import("../src/autoreply.js");

const tick = () => new Promise((r) => setTimeout(r, 0));
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((res, rej) => ((resolve = res), (reject = rej)));
  return { promise, resolve, reject };
};

const TEMPLATES = [
  { id: "adr", name: "ADR reply", description: "", payload: "MSH|^~\\&|S|{{NOW}}||ADR^A19|{{CONTROL_ID}}\rMSA|AA|{{REQ_CONTROL_ID}}", variables: [{ name: "REQ_CONTROL_ID", default: "" }] },
  { id: "nf", name: "Not found", description: "", payload: "NF", variables: [] },
];

/** Fake backend: the persisted auto reply configuration. */
let backend;
function resetBackend(config = { enabled: false, rules: [] }, present = true) {
  backend = { config: structuredClone(config), present, failSave: null, saveGate: null };
}
ipc.handler = async (cmd, args) => {
  if (cmd === "load_auto_reply") return { config: structuredClone(backend.config), present: backend.present };
  if (cmd === "save_auto_reply") {
    if (backend.saveGate) await backend.saveGate.promise;
    if (backend.failSave) throw backend.failSave;
    backend.config = structuredClone(args.config);
    backend.present = true;
    return undefined;
  }
  if (cmd === "set_auto_reply_enabled") {
    if (backend.toggleGate) await backend.toggleGate.promise;
    if (backend.failToggle) throw backend.failToggle;
    backend.config.enabled = args.enabled;
    return undefined;
  }
  return undefined;
};

const calls = (cmd) => ipc.calls.filter((c) => c.cmd === cmd);
const rows = () => getNode("ar-list").children.filter((c) => c.classes.has("ar-item"));
const rowText = (i) => rows()[i].text;
const bannerText = () => getNode("ar-banner-text").textContent;
const errorsText = () => getNode("ar-errors").text;

let started = false;
/** Loads from the current fake backend with a clean UI state. */
async function fresh(config, present = true, templates = TEMPLATES) {
  resetBackend(config, present);
  ipc.calls = [];
  clearSavedTemplates();
  if (templates) publishSavedTemplates(templates);
  if (!started) {
    started = true;
    for (const fn of winListeners.DOMContentLoaded ?? []) await fn();
  } else {
    getNode("ar-guard-cancel").fire("click");
    getNode("ar-discard").fire("click");
    await tick();
    getNode("ar-banner-retry").fire("click");
    await tick();
  }
  showView("autoreply", { force: true });
  ipc.calls = [];
}

const addRule = (patch = {}) => {
  getNode("ar-add").fire("click");
  const set = {
    "ar-name": patch.name,
    "ar-type": patch.type,
    "ar-delay": patch.delay,
  };
  for (const [id, value] of Object.entries(set)) if (value !== undefined) getNode(id).type(value);
  if (patch.template) getNode("ar-template").choose(patch.template);
};

const PERSISTED_RULE = {
  id: "r-1",
  name: "Query",
  enabled: true,
  trigger: { type: "hl7", message_type: "QRY^A19" },
  condition: { segment: "QRD", field: 8, operator: "glob", value: "AAZ*" },
  action: { type: "template", template_id: "adr" },
  delay_ms: 50,
};

test("load shows rules, count, applied master state and badge; no default acknowledgement control exists", async () => {
  await fresh({ enabled: true, rules: [PERSISTED_RULE] });
  assert.equal(rows().length, 1);
  assert.match(rowText(0), /Query/);
  assert.match(rowText(0), /HL7 QRY\^A19/);
  assert.match(rowText(0), /QRD-8 matches AAZ\*/);
  assert.match(rowText(0), /Template: ADR reply/);
  assert.match(rowText(0), /50 ms/);
  assert.equal(getNode("ar-enabled").checked, true);
  assert.equal(getNode("autoreply-badge").hidden, false);
  assert.equal(getNode("ar-save").disabled, true, "clean state");
  assert.equal(calls("set_auto_reply_enabled").length, 0);
  assert.equal(calls("update_auto_response").length, 0, "no startup push of any kind");
});

test("a failed load blocks add, save and the switch; Retry recovers without writing", async () => {
  await fresh();
  ipc.handler = async (cmd) => {
    throw new Error(`broken ${cmd}`);
  };
  getNode("ar-banner-retry").fire("click");
  await tick();
  assert.match(bannerText(), /Could not load the auto reply rules/);
  assert.equal(getNode("ar-banner-retry").hidden, false);
  assert.equal(getNode("ar-add").disabled, true);
  assert.equal(getNode("ar-enabled").disabled, true);
  assert.equal(getNode("autoreply-badge").hidden, true);
  getNode("ar-add").fire("click");
  getNode("ar-save").fire("click");
  getNode("ar-enabled").choose(true);
  await tick();
  assert.equal(rows().length, 0);
  assert.equal(calls("save_auto_reply").length, 0);
  assert.equal(calls("set_auto_reply_enabled").length, 0);
  assert.equal(getNode("ar-enabled").checked, false);

  resetBackend({ enabled: false, rules: [PERSISTED_RULE] });
  ipc.handler = backendHandler;
  getNode("ar-banner-retry").fire("click");
  await tick();
  assert.equal(rows().length, 1);
  assert.equal(getNode("ar-add").disabled, false);
  assert.equal(calls("save_auto_reply").length, 0);
});
const backendHandler = ipc.handler;

test("add, edit, validate, save: persisted shape is exact and the draft becomes clean", async () => {
  await fresh();
  addRule({ name: "Answer ORU", type: "ORU^R01", delay: "200", template: "nf" });
  assert.equal(rows().length, 1);
  assert.equal(getNode("ar-dirty").hidden, false);
  assert.equal(getNode("ar-save").disabled, false);
  getNode("ar-cond-use").choose(true);
  getNode("ar-cond-seg").type("PID");
  getNode("ar-cond-field").type("3");
  getNode("ar-cond-value").type("123*");
  getNode("ar-cond-op").choose("glob");
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply").length, 1);
  const saved = calls("save_auto_reply")[0].args.config;
  assert.equal(saved.enabled, false);
  assert.equal(saved.rules.length, 1);
  assert.deepEqual(
    { ...saved.rules[0], id: "x" },
    {
      id: "x",
      name: "Answer ORU",
      enabled: true,
      trigger: { type: "hl7", message_type: "ORU^R01" },
      condition: { segment: "PID", field: 3, operator: "glob", value: "123*" },
      action: { type: "template", template_id: "nf" },
      delay_ms: 200,
    },
  );
  assert.equal(getNode("ar-dirty").hidden, true);
  assert.equal(getNode("ar-save").disabled, true);
  assert.match(bannerText(), /Saved 1 rule/);
});

test("switching the action stores the literal or the generated acknowledgement in the rule", async () => {
  await fresh();
  addRule({ name: "Frame", delay: "0" });
  getNode("ar-trigger").choose("astm_frame");
  getNode("ar-action").choose("literal");
  getNode("ar-literal").type("<ACK>");
  assert.equal(getNode("ar-hl7-fields").hidden, true);
  assert.equal(getNode("ar-act-literal").hidden, false);
  assert.equal(getNode("ar-act-template").hidden, true);
  addRule({ name: "Acks", type: "ORU^R01" });
  getNode("ar-action").choose("hl7_ack");
  getNode("ar-ack-type").type("ACK^R01");
  getNode("ar-ack-code").type("AE");
  getNode("ar-save").fire("click");
  await tick();
  const [a, b] = calls("save_auto_reply")[0].args.config.rules;
  assert.deepEqual(a.trigger, { type: "astm_frame" });
  assert.deepEqual(a.action, { type: "literal", text: "<ACK>" });
  assert.deepEqual(b.action, { type: "hl7_ack", message_type: "ACK^R01", code: "AE" });
});

test("No auto reply: hides the reply fields, saves {type:none} with delay 0, and switching back validates again", async () => {
  await fresh({ enabled: true, rules: [PERSISTED_RULE] });
  getNode("ar-list").children.find((c) => c.classes.has("ar-item")).fire("click");
  getNode("ar-delay").type("abc");
  getNode("ar-action").choose("none");
  assert.equal(getNode("ar-act-none").hidden, false);
  for (const id of ["ar-act-template", "ar-act-literal", "ar-act-ack", "ar-delay-field"]) {
    assert.equal(getNode(id).hidden, true, id);
  }
  assert.equal(errorsText(), "");
  assert.match(rowText(0), /No auto reply/);
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply").length, 1);
  assert.deepEqual(backend.config.rules[0].action, { type: "none" });
  assert.equal(backend.config.rules[0].delay_ms, 0);
  assert.equal(backend.config.enabled, true);

  // Reload: the saved rule comes back as No auto reply with the canonical delay.
  await fresh(backend.config);
  getNode("ar-list").children.find((c) => c.classes.has("ar-item")).fire("click");
  assert.equal(getNode("ar-action").value, "none");
  assert.equal(getNode("ar-delay").value, "0");

  // Switching back to a reply action shows its fields and validates them again.
  getNode("ar-action").choose("literal");
  assert.equal(getNode("ar-act-none").hidden, true);
  assert.equal(getNode("ar-delay-field").hidden, false);
  assert.match(errorsText(), /Type the text to send/);
  getNode("ar-save").fire("click");
  await tick();
  assert.deepEqual(backend.config.rules[0].action, { type: "none" });
});

test("rule toggle, ordering and two-step delete", async () => {
  const mk = (id, name) => ({ ...PERSISTED_RULE, id, name, condition: null });
  await fresh({ enabled: false, rules: [mk("a", "A"), mk("b", "B"), mk("c", "C")] });
  const tools = (i) => rows()[i].children.at(-1).children;
  tools(2)[0].fire("click"); // C up
  assert.deepEqual(rows().map((r) => r.text.match(/[ABC]/)[0]), ["A", "C", "B"]);
  assert.equal(tools(0)[0].disabled, true, "first rule cannot move up");
  tools(0)[1].fire("click"); // A down
  assert.deepEqual(rows().map((r) => r.text.match(/[ABC]/)[0]), ["C", "A", "B"]);

  rows()[1].children[0].children[0].fire("change"); // toggle A off
  assert.equal(rows()[1].classes.has("off"), true);

  tools(2)[2].fire("click"); // arm delete B
  assert.equal(rows().length, 3, "first click only arms");
  assert.equal(tools(2)[2].textContent, "Sure?");
  tools(2)[2].fire("click");
  assert.equal(rows().length, 2);

  getNode("ar-save").fire("click");
  await tick();
  const saved = calls("save_auto_reply")[0].args.config.rules;
  assert.deepEqual(saved.map((r) => [r.id, r.enabled]), [["c", true], ["a", false]]);
});

test("invalid rules are refused with visible per-field errors and nothing is written", async () => {
  await fresh();
  addRule({ name: "Bad", type: "QRY A19", delay: "70000" });
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply").length, 0);
  assert.match(bannerText(), /Nothing was saved/);
  assert.match(errorsText(), /message type/i);
  assert.match(errorsText(), /Choose the template/);
  assert.match(errorsText(), /between 0 and 60000/);
  assert.equal(getNode("ar-type").attrs["aria-invalid"], "true");
  assert.match(rowText(0), /!/);
  assert.equal(getNode("ar-dirty").hidden, false, "draft kept");
});

test("template dropdown shows only SAVED templates and follows library events", async () => {
  await fresh();
  const names = () => getNode("ar-template").children.map((o) => o.textContent);
  addRule({ name: "T" });
  assert.deepEqual(names(), ["Choose a template…", "ADR reply", "Not found"]);
  getNode("ar-template").choose("adr");
  assert.match(getNode("ar-preview").text, /ADR\^A19/);
  assert.match(getNode("ar-preview").text, /MSA\|AA\|‹request MSH-10›/);
  assert.match(getNode("ar-vars").text, /REQ_CONTROL_ID.*MSH-10 of the received message/);

  publishSavedTemplates([...TEMPLATES, { id: "n3", name: "Third", description: "", payload: "X", variables: [] }]);
  assert.deepEqual(names(), ["Choose a template…", "ADR reply", "Not found", "Third"]);
  publishSavedTemplates([TEMPLATES[1]]); // adr deleted
  assert.deepEqual(names().slice(-1), ["(missing) adr"]);
  assert.match(errorsText(), /no longer exists/);
  assert.match(rowText(0), /\(missing\)/);
  clearSavedTemplates();
  assert.deepEqual(names().slice(0, 1), ["Template library unavailable"]);
  assert.equal(calls("save_templates").length, 0, "Rules never write templates");
});

test("a template that cannot be used as an automatic response is rejected before save", async () => {
  const broken = { id: "bad", name: "Bad", description: "", payload: "A {{X}}", variables: [{ name: "X", default: "" }] };
  await fresh({ enabled: false, rules: [] }, true, [...TEMPLATES, broken]);
  addRule({ name: "Uses bad", type: "*", template: "bad" });
  assert.match(errorsText(), /Unresolved variable\(s\): X/);
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply").length, 0);
});

test("the master switch persists only the enabled flag, never the dirty draft", async () => {
  await fresh({ enabled: false, rules: [PERSISTED_RULE] });
  addRule({ name: "Half edited", type: "" });
  assert.equal(getNode("ar-dirty").hidden, false);
  getNode("ar-enabled").choose(true);
  await tick();
  assert.deepEqual(calls("set_auto_reply_enabled").map((c) => c.args), [{ enabled: true }]);
  assert.equal(calls("save_auto_reply").length, 0);
  assert.equal(backend.config.rules.length, 1, "persisted rules untouched");
  assert.equal(getNode("autoreply-badge").hidden, false);
  assert.equal(getNode("ar-dirty").hidden, false, "draft still unsaved");

  getNode("ar-save").fire("click"); // draft invalid: refused
  await tick();
  assert.equal(calls("save_auto_reply").length, 0);
  getNode("ar-type").type("ORU^R01");
  getNode("ar-template").choose("nf");
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply")[0].args.config.enabled, true, "a rules save keeps the applied switch");
});

test("a failed switch change is reported and the control returns to the applied state", async () => {
  await fresh({ enabled: false, rules: [] });
  backend.failToggle = "disk full";
  getNode("ar-enabled").choose(true);
  await tick();
  assert.match(bannerText(), /Could not enable automatic replies: disk full/);
  assert.equal(getNode("ar-enabled").checked, false);
  assert.equal(getNode("autoreply-badge").hidden, true);
});

test("a pending switch change refuses every rules save, then later saves carry the confirmed flag", async () => {
  for (const initial of [false, true]) {
    for (const via of ["save", "guard"]) {
      await fresh({ enabled: initial, rules: [PERSISTED_RULE] });
      addRule({ name: "Pending draft", type: "*", template: "nf" });
      backend.toggleGate = deferred();
      getNode("ar-enabled").choose(!initial);
      await tick();
      assert.deepEqual(calls("set_auto_reply_enabled").map((c) => c.args), [{ enabled: !initial }]);
      assert.equal(getNode("ar-enabled").disabled, true);
      assert.equal(getNode("ar-save").disabled, true, "Save disabled while the switch is pending");
      assert.equal(getNode("ar-discard").disabled, true);
      assert.equal(getNode("autoreply-badge").hidden, !initial, "badge shows only the confirmed state");

      if (via === "save") {
        getNode("ar-save").fire("click");
      } else {
        assert.equal(showView("home"), false);
        assert.equal(getNode("ar-guard").hidden, false);
        assert.equal(getNode("ar-guard-save").disabled, true);
        assert.equal(getNode("ar-guard-discard").disabled, true);
        getNode("ar-guard-save").fire("click");
        getNode("ar-guard-discard").fire("click");
      }
      await tick();
      assert.equal(calls("save_auto_reply").length, 0, `no conflicting rules write (${via}, initial ${initial})`);
      assert.equal(getNode("ar-dirty").hidden, false, "dirty draft kept");
      assert.equal(rows().length, 2);
      assert.equal(currentView(), "autoreply", "no navigation bypass");
      if (via === "guard") assert.equal(getNode("ar-guard").hidden, false);

      backend.toggleGate.resolve();
      await tick();
      await tick();
      assert.equal(backend.config.enabled, !initial);
      assert.equal(getNode("ar-enabled").checked, !initial);
      assert.equal(getNode("ar-save").disabled, false);
      assert.equal(getNode("ar-dirty").hidden, false, "draft still unsaved after confirmation");

      if (via === "save") getNode("ar-save").fire("click");
      else getNode("ar-guard-save").fire("click");
      await tick();
      assert.equal(calls("save_auto_reply").length, 1);
      assert.equal(calls("save_auto_reply")[0].args.config.enabled, !initial, "save carries the confirmed flag");
      assert.equal(backend.config.enabled, !initial);
      assert.equal(backend.config.rules.length, 2);
      if (via === "guard") assert.equal(currentView(), "home");
      showView("autoreply", { force: true });
      backend.toggleGate = null;
    }
  }
});

test("a failed pending switch change restores the confirmed state and allows a save", async () => {
  await fresh({ enabled: false, rules: [PERSISTED_RULE] });
  addRule({ name: "Draft", type: "*", template: "nf" });
  backend.toggleGate = deferred();
  backend.failToggle = "disk full";
  getNode("ar-enabled").choose(true);
  await tick();
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply").length, 0);
  backend.toggleGate.resolve();
  await tick();
  await tick();
  assert.match(bannerText(), /Could not enable automatic replies: disk full/);
  assert.equal(getNode("ar-enabled").checked, false);
  assert.equal(getNode("autoreply-badge").hidden, true);
  assert.equal(getNode("ar-dirty").hidden, false);
  backend.toggleGate = null;
  backend.failToggle = null;
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply").length, 1);
  assert.equal(calls("save_auto_reply")[0].args.config.enabled, false);
});

test("legacy settings are imported as an unsaved, disabled draft and removed only after a successful save", async () => {
  store.set("simautomate:config", JSON.stringify({ "autoresponse-enabled": true, astm_ack: "<ACK>", hl7_type: "ACK^O21", hl7_code: "AA", theme: "x" }));
  await fresh({ enabled: false, rules: [] }, false);
  assert.equal(rows().length, 3);
  assert.match(bannerText(), /Imported previous settings — review and Save/);
  assert.equal(getNode("ar-banner").classes.has("warn"), true);
  assert.equal(getNode("ar-enabled").checked, false);
  assert.equal(getNode("autoreply-badge").hidden, true);
  assert.equal(getNode("ar-dirty").hidden, false);
  assert.equal(calls("save_auto_reply").length, 0, "importing writes nothing");
  assert.ok(store.get("simautomate:config").includes("astm_ack"), "legacy keys kept until saved");

  backend.failSave = "denied";
  getNode("ar-save").fire("click");
  await tick();
  assert.ok(store.get("simautomate:config").includes("astm_ack"), "failed save keeps legacy keys");
  assert.equal(getNode("ar-dirty").hidden, false);

  backend.failSave = null;
  getNode("ar-save").fire("click");
  await tick();
  assert.deepEqual(JSON.parse(store.get("simautomate:config")), { theme: "x" });
  const saved = calls("save_auto_reply").at(-1).args.config;
  assert.equal(saved.enabled, false);
  assert.equal(saved.rules.length, 3);
  assert.deepEqual(saved.rules[0].action, { type: "literal", text: "<ACK>" });
  assert.deepEqual(saved.rules[2].action, { type: "hl7_ack", message_type: "ACK^O21", code: "AA" });
  store.clear();
});

test("corrupt legacy settings are reported, left untouched and import nothing", async () => {
  store.set("simautomate:config", "{not json");
  await fresh({ enabled: false, rules: [] }, false);
  assert.equal(rows().length, 0);
  assert.match(bannerText(), /previous settings are corrupted.*left untouched/);
  assert.equal(store.get("simautomate:config"), "{not json");
  addRule({ name: "New", type: "*", template: "nf" });
  getNode("ar-save").fire("click");
  await tick();
  assert.equal(store.get("simautomate:config"), "{not json", "never rewritten by a rules save");
  store.clear();
});

test("leaving with unsaved rules asks Save / Discard / Cancel", async () => {
  await fresh({ enabled: false, rules: [PERSISTED_RULE] });
  addRule({ name: "Pending", type: "*", template: "nf" });
  assert.equal(showView("home"), false);
  assert.equal(currentView(), "autoreply");
  assert.equal(getNode("ar-guard").hidden, false);

  getNode("ar-guard-cancel").fire("click");
  assert.equal(getNode("ar-guard").hidden, true);
  assert.equal(rows().length, 2, "cancel keeps the draft");

  assert.equal(showView("home"), false);
  getNode("ar-guard-save").fire("click");
  await tick();
  assert.equal(calls("save_auto_reply").length, 1);
  assert.equal(currentView(), "home", "navigation resumes after a successful save");

  showView("autoreply", { force: true });
  addRule({ name: "Throwaway", type: "*", template: "nf" });
  assert.equal(showView("session"), false);
  getNode("ar-guard-discard").fire("click");
  await tick();
  assert.equal(currentView(), "session");
  assert.equal(calls("save_auto_reply").length, 1, "discard writes nothing");
  assert.equal(rows().length, 2, "reloaded from the persisted rules");
  showView("autoreply", { force: true });
});

test("a failed save from the guard keeps the user on the view with the draft", async () => {
  await fresh();
  addRule({ name: "X", type: "*", template: "nf" });
  assert.equal(showView("home"), false);
  backend.failSave = "nope";
  getNode("ar-guard-save").fire("click");
  await tick();
  assert.equal(currentView(), "autoreply");
  assert.equal(getNode("ar-guard").hidden, false);
  assert.match(bannerText(), /Could not save: nope.*previous rules stay active/);
  assert.equal(rows().length, 1);
  getNode("ar-guard-cancel").fire("click");
});

test("beforeunload is blocked while the draft is dirty", async () => {
  await fresh();
  const fire = () => {
    const ev = { prevented: false, preventDefault() { this.prevented = true; } };
    for (const fn of winListeners.beforeunload ?? []) fn(ev);
    return ev.prevented;
  };
  assert.equal(fire(), false);
  addRule({ name: "Dirty", type: "*", template: "nf" });
  assert.equal(fire(), true);
});

test("an async save locks the draft: no duplicate save, no edits, no reorder, no add; completes with the written snapshot", async () => {
  await fresh();
  addRule({ name: "Locked", type: "ORU^R01", template: "nf" });
  backend.saveGate = deferred();
  getNode("ar-save").fire("click");
  getNode("ar-save").fire("click");
  assert.equal(calls("save_auto_reply").length, 1, "no duplicate save");
  assert.equal(getNode("ar-save").disabled, true);
  assert.equal(getNode("ar-add").disabled, true);
  assert.equal(getNode("ar-name").disabled, true);
  getNode("ar-name").type("Changed during save");
  getNode("ar-add").fire("click");
  assert.equal(rows().length, 1);
  assert.equal(showView("home"), false, "navigation refused while saving");

  backend.saveGate.resolve();
  backend.saveGate = null;
  await tick();
  assert.equal(calls("save_auto_reply")[0].args.config.rules[0].name, "Locked");
  assert.equal(getNode("ar-dirty").hidden, true);
  assert.equal(getNode("ar-name").value, "Locked");
  assert.equal(getNode("ar-name").disabled, false);
});

test("removing the last rules leaves an explicit empty configuration (no hidden fallback)", async () => {
  await fresh({ enabled: true, rules: [PERSISTED_RULE] });
  const tools = rows()[0].children.at(-1).children;
  tools[2].fire("click");
  tools[2].fire("click");
  assert.equal(rows().length, 0);
  assert.equal(getNode("ar-empty").hidden, false);
  getNode("ar-save").fire("click");
  await tick();
  assert.deepEqual(calls("save_auto_reply")[0].args.config, { enabled: true, rules: [] });
});
