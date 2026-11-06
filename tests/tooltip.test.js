import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { connectionDetails, describeEndpoint } from "../src/home/recent-core.js";

// el() (render.js) builds the rows through the global document.
class FakeNode {
  constructor(tag = "div") {
    this.tag = tag;
    this.children = [];
    this.attrs = {};
    this.listeners = {};
    this.classes = new Set();
    this.style = {};
    this.textContent = "";
    this.isConnected = true;
    this.rect = { left: 0, top: 0, right: 0, bottom: 0 };
    this.classList = {
      add: (c) => this.classes.add(c),
      remove: (c) => this.classes.delete(c),
      contains: (c) => this.classes.has(c),
    };
  }
  set className(v) {
    this.classes = new Set(String(v).split(/\s+/).filter(Boolean));
  }
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  fire(type, event = {}) {
    for (const fn of this.listeners[type] ?? []) fn(event);
  }
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
    return this.size ?? this.rect;
  }
  contains(node) {
    return node === this || this.children.some((c) => c.contains?.(node));
  }
}
globalThis.document = { createElement: (tag) => new FakeNode(tag) };
const { placeTooltip, createTooltipController, TOOLTIP_ID } = await import("../src/tooltip.js");

const texts = (tip) => tip.children[0].children.map((n) => n.textContent);

function setup() {
  const tip = new FakeNode();
  tip.size = { width: 200, height: 80 };
  const doc = new FakeNode();
  const controller = createTooltipController({ getElement: () => tip, getViewport: () => ({ width: 1000, height: 600 }), doc });
  const row = (x = 100, y = 100) => {
    const anchor = new FakeNode();
    anchor.rect = { left: x, top: y, right: x + 150, bottom: y + 30 };
    anchor.describes = new FakeNode("button");
    return anchor;
  };
  return { tip, doc, controller, row };
}
const rowsOf = (label = "10.0.0.1") => connectionDetails({ mode: "client", host: label, port: 6000 });
const bind = (controller, anchor, extra = {}) =>
  controller.bind(anchor, { key: "k", rows: rowsOf(), describes: anchor.describes, prefer: "bottom", ...extra });

// ── Formatting ──

const asText = (rows) => rows.map((r) => `${r.label}: ${r.value}`);

test("a client shows its type, host and port", () => {
  assert.deepEqual(asText(connectionDetails({ mode: "client", host: "LIS.local", port: "2575" })), [
    "Type: Client",
    "Host: lis.local",
    "Port: 2575",
  ]);
});

test("a server shows its listening port and no host", () => {
  assert.deepEqual(asText(connectionDetails({ mode: "server", host: "ignored", port: 5001 })), [
    "Type: Server",
    "Listening port: 5001",
  ]);
});

test("SSH connections add the SSH host, port and user", () => {
  assert.deepEqual(
    asText(connectionDetails({ mode: "ssh-client", host: "10.0.0.2", port: 2575, sshHost: "vm", sshPort: 22, sshUser: "lis" })),
    ["Type: SSH Client", "Host: 10.0.0.2", "Port: 2575", "SSH host: vm", "SSH port: 22", "SSH user: lis"],
  );
  assert.deepEqual(
    asText(connectionDetails({ mode: "ssh-server", port: 5002, sshHost: "vm", sshPort: 2222, sshUser: "" })),
    ["Type: SSH Server", "Listening port: 5002", "SSH host: vm", "SSH port: 2222", "SSH user: (default)"],
  );
});

test("a custom name comes first and the address stays visible", () => {
  const endpoint = { mode: "client", host: "lis.local", port: 2575 };
  const rows = connectionDetails(endpoint, "  Lab   LIS ");
  assert.deepEqual(asText(rows).slice(0, 4), ["Name: Lab LIS", "Type: Client", "Host: lis.local", "Port: 2575"]);
  assert.equal(describeEndpoint(endpoint), "lis.local:2575");
  assert.equal(asText(connectionDetails(endpoint, "")).some((r) => r.startsWith("Name")), false);
});

test("secrets and unknown fields are never shown", () => {
  const rows = connectionDetails(
    {
      mode: "ssh-client",
      host: "h",
      port: 1,
      sshHost: "vm",
      sshPort: 22,
      sshUser: "u",
      password: "hunter2",
      sshPassword: "hunter3",
      passphrase: "hunter4",
      req: { ssh_password: "hunter5" },
      token: "hunter6",
    },
    "Named",
  );
  assert.doesNotMatch(JSON.stringify(rows), /hunter/);
  assert.deepEqual(
    rows.map((r) => r.label),
    ["Name", "Type", "Host", "Port", "SSH host", "SSH port", "SSH user"],
  );
});

test("an unusable endpoint has no details", () => {
  assert.deepEqual(connectionDetails(null), []);
  assert.deepEqual(connectionDetails({ mode: "client", host: "", port: 1 }), []);
});

// ── Placement ──

const viewport = { width: 1000, height: 600 };
const size = { width: 200, height: 80 };
const at = (anchor, prefer) => placeTooltip({ anchor, size, viewport, prefer });

test("the tooltip goes on the preferred side when it fits", () => {
  const anchor = { left: 100, top: 100, right: 250, bottom: 130 };
  assert.deepEqual(at(anchor, "bottom"), { side: "bottom", left: 100, top: 138 });
  assert.deepEqual(at(anchor, "right"), { side: "right", left: 258, top: 100 });
});

test("it flips to the opposite side near an edge", () => {
  assert.equal(at({ left: 100, top: 540, right: 250, bottom: 570 }, "bottom").side, "top");
  assert.equal(at({ left: 100, top: 540, right: 250, bottom: 570 }, "bottom").top, 540 - 8 - 80);
  assert.equal(at({ left: 800, top: 100, right: 990, bottom: 130 }, "right").side, "left");
});

test("it is clamped inside the window when nothing fits", () => {
  const huge = { left: -50, top: -20, right: 1100, bottom: 700 };
  const placed = at(huge, "bottom");
  assert.ok(placed.left >= 8 && placed.left + size.width <= 992);
  assert.ok(placed.top >= 8 && placed.top + size.height <= 592);
  const edge = at({ left: 950, top: 100, right: 1000, bottom: 130 }, "bottom");
  assert.equal(edge.left, 1000 - 200 - 8, "kept inside on the right edge");
});

// ── Controller ──

test("hover shows the tooltip with the rows and links it to the row", () => {
  const { tip, controller, row } = setup();
  const anchor = row();
  bind(controller, anchor);
  anchor.fire("mouseenter");
  assert.equal(tip.classes.has("show"), true);
  assert.deepEqual(texts(tip), ["Type", "Client", "Host", "10.0.0.1", "Port", "6000"]);
  assert.equal(anchor.describes.attrs["aria-describedby"], TOOLTIP_ID);
  assert.equal(tip.style.left, "100px");
  assert.equal(tip.style.top, "138px");

  anchor.fire("mouseleave");
  assert.equal(tip.classes.has("show"), false);
  assert.equal("aria-describedby" in anchor.describes.attrs, false);
});

test("keyboard focus shows it and leaving the row hides it, but moving between its buttons does not", () => {
  const { tip, controller, row } = setup();
  const anchor = row();
  const inner = new FakeNode("button");
  anchor.appendChild(inner);
  bind(controller, anchor);
  anchor.fire("focusin", { target: { matches: () => true } });
  assert.equal(tip.classes.has("show"), true);
  anchor.fire("focusout", { relatedTarget: inner });
  assert.equal(tip.classes.has("show"), true, "focus stays inside the row");
  anchor.fire("focusout", { relatedTarget: null });
  assert.equal(tip.classes.has("show"), false);
});

test("a mouse click focus (not focus-visible) does not pin the tooltip", () => {
  const { tip, controller, row } = setup();
  const anchor = row();
  bind(controller, anchor);
  anchor.fire("focusin", { target: { matches: () => false } });
  assert.equal(tip.classes.has("show"), false);
});

test("hover keeps it up while focus has left, and the reverse", () => {
  const { tip, controller, row } = setup();
  const anchor = row();
  bind(controller, anchor);
  anchor.fire("mouseenter");
  anchor.fire("focusin", { target: { matches: () => true } });
  anchor.fire("mouseleave");
  assert.equal(tip.classes.has("show"), true);
  anchor.fire("focusout", {});
  assert.equal(tip.classes.has("show"), false);
});

test("Escape hides it without consuming the key, and hovering again shows it", () => {
  const { tip, doc, controller, row } = setup();
  const anchor = row();
  bind(controller, anchor);
  anchor.fire("mouseenter");
  let prevented = false;
  doc.fire("keydown", { key: "Escape", preventDefault: () => (prevented = true), stopPropagation: () => (prevented = true) });
  assert.equal(tip.classes.has("show"), false);
  assert.equal(prevented, false);
  anchor.fire("mouseleave");
  anchor.fire("mouseenter");
  assert.equal(tip.classes.has("show"), true);
});

test("other keys leave it alone", () => {
  const { tip, doc, controller, row } = setup();
  const anchor = row();
  bind(controller, anchor);
  anchor.fire("mouseenter");
  doc.fire("keydown", { key: "a" });
  assert.equal(tip.classes.has("show"), true);
});

test("it opens inside the window near the bottom edge", () => {
  const { tip, controller, row } = setup();
  const anchor = row(900, 560);
  bind(controller, anchor, { prefer: "right" });
  anchor.fire("mouseenter");
  assert.equal(tip.style.left, `${900 - 8 - 200}px`);
  assert.equal(tip.style.top, "512px", "clamped to the bottom margin (600 - 80 - 8)");
});

test("a re-rendered row keeps the tooltip and refreshes its rows (rename while visible)", () => {
  const { tip, controller, row } = setup();
  const old = row();
  bind(controller, old);
  old.fire("mouseenter");

  old.isConnected = false; // the list was rebuilt
  const next = row();
  next.matches = (selector) => selector === ":hover";
  bind(controller, next, { rows: connectionDetails({ mode: "client", host: "10.0.0.1", port: 6000 }, "Lab") });
  controller.sync();

  assert.equal(tip.classes.has("show"), true);
  assert.deepEqual(texts(tip).slice(0, 2), ["Name", "Lab"]);
  assert.equal(next.describes.attrs["aria-describedby"], TOOLTIP_ID);
  assert.equal("aria-describedby" in old.describes.attrs, false);

  next.fire("mouseleave");
  assert.equal(tip.classes.has("show"), false);
});

test("it hides when its row disappears or is no longer under the pointer", () => {
  const { tip, controller, row } = setup();
  const old = row();
  bind(controller, old);
  old.fire("mouseenter");
  old.isConnected = false;
  controller.sync();
  assert.equal(tip.classes.has("show"), false);

  const again = row();
  bind(controller, again);
  again.fire("mouseenter");
  again.isConnected = false;
  const rebuilt = row();
  rebuilt.matches = () => false;
  bind(controller, rebuilt);
  controller.sync();
  assert.equal(tip.classes.has("show"), false);
});

test("an entry without rows never shows an empty tooltip", () => {
  const { tip, controller, row } = setup();
  const anchor = row();
  bind(controller, anchor, { rows: [] });
  anchor.fire("mouseenter");
  assert.equal(tip.classes.has("show"), false);
});

// ── Action-button hints ──

const hintOf = (tip) => tip.children[0].textContent;

test("a hint is text only, placed on its preferred side and kept inside the window", () => {
  const { tip, controller, row } = setup();
  const button = row(100, 5);
  controller.bindHint(button, { key: "h", text: "Close this session", prefer: "bottom" });
  button.fire("mouseenter");
  assert.equal(tip.classes.has("show"), true);
  assert.equal(tip.classes.has("text"), true);
  assert.equal(hintOf(tip), "Close this session");
  assert.equal(tip.style.top, "43px", "below the button, not above the window");

  button.fire("mouseleave");
  const low = row(100, 560);
  controller.bindHint(low, { key: "l", text: "Rename", prefer: "bottom" });
  low.fire("mouseenter");
  assert.equal(tip.style.top, `${560 - 8 - 80}px`, "flipped above near the bottom");
});

test("a hint reads its text when shown, shows on keyboard focus, and hides on click and Escape", () => {
  const { tip, doc, controller, row } = setup();
  const button = row();
  let label = "Copy";
  controller.bindHint(button, { key: "h", text: () => label });
  button.fire("focusin", { target: { matches: () => true } });
  assert.equal(hintOf(tip), "Copy");
  button.fire("focusout", {});
  label = "Copied";
  button.fire("mouseenter");
  assert.equal(hintOf(tip), "Copied");
  button.fire("click");
  assert.equal(tip.classes.has("show"), false);
  button.fire("mouseleave");
  button.fire("mouseenter");
  doc.fire("keydown", { key: "Escape" });
  assert.equal(tip.classes.has("show"), false);
});

test("the row tooltip yields to a button hint and returns when leaving the button", () => {
  const { tip, controller, row } = setup();
  const anchor = row();
  const button = new FakeNode("button");
  button.rect = { left: 120, top: 100, right: 150, bottom: 120 };
  anchor.appendChild(button);
  bind(controller, anchor);
  controller.bindHint(button, { key: "h", text: "Rename" });

  anchor.fire("mouseenter");
  button.fire("mouseenter");
  assert.equal(hintOf(tip), "Rename");
  assert.equal("aria-describedby" in anchor.describes.attrs, false);
  button.fire("mouseleave");
  assert.equal(tip.classes.has("text"), false);
  assert.deepEqual(texts(tip).slice(0, 2), ["Type", "Client"]);
  anchor.fire("mouseleave");
  assert.equal(tip.classes.has("show"), false);
});

test("action buttons carry no native title or data-tooltip, only accessible names", () => {
  for (const file of ["home/recent-card.html", "home/recent-side-item.html", "session/session-item.html", "session/session.html"]) {
    const html = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
    const buttons = html.match(/<button[^>]*(?:ses-rename|ses-close|card-copy|card-rename|id="session-(?:rename|close)")[^>]*>/g) ?? [];
    assert.ok(buttons.length > 0, file);
    for (const b of buttons) assert.doesNotMatch(b, /title=|data-tooltip/, b);
  }
  for (const file of ["home/connection.js", "session/session-ui.js"]) {
    const js = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
    assert.doesNotMatch(js, /(rename|close|copy)\.title\s*=|Rename\.title/i, file);
  }
});

test("new session, control-character chips and composer Autobuild use hints, not title", () => {
  const read = (file) => readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
  const index = read("index.html");
  assert.doesNotMatch(index.match(/<button[^>]*id="session-new"[^>]*>/)?.[0] ?? "title=", /title=|data-tooltip/);
  const session = read("session/session.html");
  const chips = session.match(/<button[^>]*class="chip"[^>]*>/g) ?? [];
  assert.equal(chips.length, 7);
  for (const b of chips) assert.match(b, /aria-label="Send /);
  const auto = session.match(/<button[^>]*id="composer-autobuild"[^>]*>/)?.[0] ?? "";
  assert.match(auto, /aria-label=/);
  for (const b of [...chips, auto]) assert.doesNotMatch(b, /title=|data-tooltip/, b);
  assert.doesNotMatch(session, /\stitle=/);
  const js = read("session/session-ui.js");
  for (const id of ["session-new", "composer-autobuild"]) assert.match(js, new RegExp(`bindHint\\(\\$\\("${id}"\\)`));
  assert.match(js, /bindHint\(chip,/);
});

// ── Markup and styles ──

test("the page has one tooltip element and the stylesheet sets text and background together", () => {
  const html = readFileSync(new URL("../src/index.html", import.meta.url), "utf8");
  assert.match(html, /id="conn-tooltip"[^>]*role="tooltip"/);
  const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
  const block = /\n\.conn-tip \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
  assert.match(block, /background: var\(--ink\)/);
  assert.match(block, /color: var\(--on\)/);
  assert.match(block, /pointer-events: none/);
});

test("recent and session items no longer carry a native title for the connection", () => {
  for (const file of ["home/recent-side-item.html", "home/recent-card.html", "session/session-item.html"]) {
    const html = readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8");
    assert.doesNotMatch(html.match(/<button[^>]*class="(?:ses|card)"[^>]*>/)?.[0] ?? "", /title=/, file);
  }
  const js = readFileSync(new URL("../src/home/connection.js", import.meta.url), "utf8");
  assert.doesNotMatch(js, /querySelector\("\.ses"\)\.title/);
});
