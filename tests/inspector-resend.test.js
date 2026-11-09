// Integration: the inspector Resend button (real sessions, messages and inspector modules on a fake DOM).
import test from "node:test";
import assert from "node:assert/strict";
import { installTemplates, bindStaticActions } from "./helpers/markup.js";
import { encodeTokens } from "../src/session/resend-core.js";

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
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
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
  replaceChildren() {
    this.children = [];
  }
  setAttribute(name, value) {
    this.attrs[name] = String(value);
  }
  removeAttribute(name) {
    delete this.attrs[name];
  }
  focus() {}
  replaceWith() {}
  get parentElement() {
    return this.parent;
  }
  toggleAttribute() {}
  contains(node) {
    return node === this || this.children.some((c) => c.contains?.(node));
  }
}

const nodes = new Map();
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
  createTextNode: (t) => Object.assign(new FakeNode("#text"), { textContent: t }),
  querySelectorAll: () => [],
  addEventListener() {},
  removeEventListener() {},
  dispatchEvent() {},
};
const ipc = { calls: [], fail: false, gate: null };
globalThis.window = {
  innerWidth: 1000,
  innerHeight: 700,
  addEventListener() {},
  removeEventListener() {},
  __TAURI__: {
    core: {
      invoke: async (cmd, args) => {
        ipc.calls.push({ cmd, args });
        if (cmd === "send_message") {
          await ipc.gate;
          if (ipc.fail) throw new Error("refused");
        }
        return undefined;
      },
    },
    event: { listen: async () => {} },
  },
  connection_status: { get: () => "disconnected", set() {}, subscribe() {} },
};

await installTemplates(["session/inspector-tab.html", "session/segment-block.html", "session/system-message.html", "session/message-bubble.html", "session/astm-badge.html", "session/astm-frame.html"]);
await bindStaticActions("session/session.html", getNode);
const { sessions, store } = await import("../src/session/sessions.js");
await import("../src/session/messages.js");

const tick = () => new Promise((r) => setTimeout(r, 0));
const button = () => getNode("insp-resend");
const sent = () => ipc.calls.filter((c) => c.cmd === "send_message").map((c) => ({ id: c.args.sessionId, message: c.args.payload.message }));
const startConnected = async (port, label) => {
  const { id, done } = sessions.start({ mode: "client", label, req: { type: "ClientConnectRequest", ip: "h", port } });
  await done;
  sessions.handleStatus({ session_id: id, attempt: store.get(id).attempt, status: "connected" });
  return id;
};
const disconnect = (id) => sessions.handleStatus({ session_id: id, attempt: store.get(id).attempt, status: "disconnected" });
const push = (id, fields) => store.applyMessage({ session_id: id, attempt: store.get(id).attempt, timestamp: "2025-01-01T00:00:00Z", ...fields });
const hl7 = (id, content = HL7, msg_type = "sent") => push(id, { msg_type, content, raw: encodeTokens(content) });
const HL7 = "<VT>MSH|^~\\&|A|B|C|D|20250101||ADT^A01|1|P|2.4<CR>PID|1<CR><FS><CR>";
const astm = (id, kind, content, extra = {}) => push(id, { msg_type: "sent", content, raw: encodeTokens(content), astm: { kind, ...extra } });
const FRAME = "<STX>1H|\\^&|||<CR><ETX>00<CR><LF>";
const frameInfo = { number: 1, text: "H|\\^&|||\r", terminator: "ETX", checksum: "00", computed_checksum: "00", checksum_ok: true };

let a, b;
test("setup: two connected sessions", async () => {
  a = await startConnected(1, "Alpha");
  b = await startConnected(2, "Beta");
  store.select(a);
  assert.equal(button().disabled, true, "nothing selected");
});

test("enabled for a sent HL7 message, disabled for received, ACK and system records", () => {
  const sentRec = hl7(a);
  store.selectRecord(a, sentRec.id);
  assert.equal(button().disabled, false);
  assert.equal(button().attrs["aria-busy"], "false");
  store.selectRecord(a, hl7(a, HL7, "received").id);
  assert.equal(button().disabled, true);
  store.selectRecord(a, push(a, { msg_type: "sent", content: "<ACK>", raw: [6] }).id);
  assert.equal(button().disabled, true);
  store.selectRecord(a, store.addLocal(a, "systemerror", "boom").id);
  assert.equal(button().disabled, true);
});

test("ASTM: disabled while open, enabled when the transfer completes (selected record refreshes)", () => {
  const open = astm(a, "enq", "<ENQ>");
  store.selectRecord(a, open.id);
  assert.equal(open.transfer.status, "open");
  assert.equal(button().disabled, true);
  astm(a, "frame", FRAME, frameInfo);
  assert.equal(button().disabled, true);
  astm(a, "eot", "<EOT>");
  assert.equal(open.transfer.status, "complete");
  assert.equal(button().disabled, false);
});

test("a click sends the stored payload to the record's own session and keeps the original", async () => {
  const rec = hl7(a);
  store.selectRecord(a, rec.id);
  store.setDraft(a, "draft text");
  const count = store.get(a).records.length;
  ipc.calls = [];
  window.actions.insp.resend();
  await tick();
  assert.deepEqual(sent(), [{ id: a, message: HL7 }]);
  assert.equal(store.get(a).draft, "draft text");
  assert.equal(store.get(a).records[count - 1], rec);
  assert.equal(button().disabled, false, "restored");
});

test("ASTM resend sends the entries joined by newlines, without replies", async () => {
  const rec = store.get(a).records.find((r) => r.transfer);
  store.selectRecord(a, rec.id);
  ipc.calls = [];
  window.actions.insp.resend();
  await tick();
  assert.deepEqual(sent(), [{ id: a, message: `<ENQ>\n${FRAME}\n<EOT>` }]);
});

test("switching session or selection during the send never redirects it; double clicks are ignored", async () => {
  const rec = hl7(a);
  store.selectRecord(a, rec.id);
  let release;
  ipc.gate = new Promise((r) => (release = r));
  ipc.calls = [];
  window.actions.insp.resend();
  assert.equal(button().disabled, true);
  assert.equal(button().attrs["aria-busy"], "true");
  window.actions.insp.resend();
  store.select(b);
  hl7(b);
  store.selectRecord(b, store.get(b).records[0].id);
  window.actions.insp.resend();
  await tick();
  release();
  ipc.gate = null;
  await tick();
  assert.deepEqual(sent(), [{ id: a, message: HL7 }], "one send, to the record's session");
  assert.equal(button().attrs["aria-busy"], "false");
  assert.equal(button().disabled, false);
});

test("uses the record's own session, not the active one, to decide", () => {
  store.select(a);
  store.selectRecord(a, hl7(a).id);
  assert.equal(button().disabled, false);
  disconnect(b);
  assert.equal(button().disabled, false, "another session going away changes nothing");
  disconnect(a);
  assert.equal(button().disabled, true, "its own session cannot send");
  const before = ipc.calls.length;
  window.actions.insp.resend();
  assert.equal(ipc.calls.length, before, "a disabled Resend does not send");
});

test("a refused send is reported in the conversation and on the inspector", async () => {
  const c = await startConnected(3, "Gamma");
  store.select(c);
  store.selectRecord(c, hl7(c).id);
  ipc.fail = true;
  window.actions.insp.resend();
  await tick();
  ipc.fail = false;
  const notices = store.get(c).records.filter((r) => r.msg_type === "systemerror");
  assert.match(notices.at(-1).content, /Failed to send/);
  assert.ok(getNode("insp-meta").children.some((n) => n.textContent === "Resend failed"));
  assert.equal(button().disabled, false, "restored after the failure");
});
