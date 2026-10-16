// include.js: data-include markup loading and file-per-template cloning, driven through a
// minimal fake DOM, plus consistency checks between index.html, the view fragments and the scripts.
import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, existsSync, statSync } from "node:fs";
import { readMarkup, installTemplates } from "./helpers/markup.js";

class El {
  constructor(tag, attrs = {}, children = []) {
    this.tag = tag;
    this.attrs = { ...attrs };
    this.children = children;
    this.dataset = {};
    this.textContent = "";
    this.removed = false;
  }
  getAttribute(name) {
    return this.attrs[name] ?? null;
  }
  setAttribute(name, value) {
    this.attrs[name] = value;
  }
  removeAttribute(name) {
    delete this.attrs[name];
  }
  replaceChildren(fragment) {
    this.children = fragment.children;
  }
  remove() {
    this.removed = true;
    if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this);
  }
  *descendants() {
    for (const child of this.children) {
      child.parent = this;
      yield child;
      yield* child.descendants();
    }
  }
  querySelectorAll(selector) {
    const match = selector.startsWith("[data-slot=")
      ? (() => {
          const names = selector.split(",").map((s) => s.slice('[data-slot="'.length, -2));
          return (n) => n.dataset.slot !== undefined && names.includes(n.dataset.slot);
        })()
      : {
          "[data-include]": (n) => "data-include" in n.attrs,
          'link[rel="x-template"]': (n) => n.tag === "link" && n.attrs.rel === "x-template",
          script: (n) => n.tag === "script",
        }[selector];
    return [...this.descendants()].filter(match);
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }
}

// The "file" served for a URL is the URL itself; PAGES turns it into elements when parsed.
const PAGES = {
  "home.html": () => [new El("p"), new El("link", { rel: "x-template", href: "row.html" })],
  "outer.html": () => [new El("div", { "data-include": "inner.html" })],
  "inner.html": () => [new El("span")],
  "shared.html": () => [new El("b")],
  "row.html": () => [new El("li")],
};
const fetched = [];
globalThis.fetch = async (url) => {
  fetched.push(url);
  return url === "missing.html"
    ? { ok: false, status: 404, text: async () => "" }
    : { ok: true, status: 200, text: async () => url };
};
const create = (tag) => new El(tag);
const createElement = (tag) =>
  tag === "template"
    ? {
        set innerHTML(html) {
          this.content = new El("#fragment", {}, PAGES[html]?.() ?? []);
        },
      }
    : create(tag);

let boots = 0;
/**
 * Runs the automatic startup of a fresh copy of include.js against a fake page and resolves with
 * the module once the includes are done (it then asks for the after-include scripts).
 */
async function boot({ hosts = [], scripts = [] } = {}) {
  const body = new El("body", {}, hosts);
  let finished;
  const done = new Promise((resolve) => (finished = resolve));
  globalThis.document = {
    baseURI: "file:///app/index.html",
    createElement,
    querySelectorAll(selector) {
      if (selector === 'script[type="after-include"]') {
        finished();
        return scripts;
      }
      return body.querySelectorAll(selector);
    },
  };
  const mod = await import(`../src/include.js?boot=${++boots}`);
  await done;
  return mod;
}
const host = (url) => new El("section", { "data-include": url });
/** A page whose only job is to preload the given template files. */
const withTemplates = (...urls) => {
  PAGES["preload.html"] = () => urls.map((href) => new El("link", { rel: "x-template", href }));
  return boot({ hosts: [host("preload.html")] });
};
const until = async (condition) => {
  for (let i = 0; i < 200 && !condition(); i++) await new Promise((r) => setTimeout(r, 5));
  assert.ok(condition(), "condition not reached in time");
};

test("an include fills the host and drops the attribute", async () => {
  const section = host("inner.html");
  await boot({ hosts: [section] });
  assert.equal(section.getAttribute("data-include"), null);
  assert.deepEqual(section.children.map((n) => n.tag), ["span"]);
});

test("nested includes are resolved", async () => {
  const section = host("outer.html");
  await boot({ hosts: [section] });
  assert.equal(section.children[0].getAttribute("data-include"), null, "inner include consumed too");
  assert.deepEqual(section.children[0].children.map((n) => n.tag), ["span"]);
});

test("a file used by several hosts is fetched once", async () => {
  fetched.length = 0;
  await boot({ hosts: [host("shared.html"), host("shared.html")] });
  assert.deepEqual(fetched, ["shared.html"]);
});

test("x-template links of an included file are preloaded and left in place", async () => {
  PAGES["home.html"] = () => [new El("p"), new El("link", { rel: "x-template", href: "row.html" })];
  const section = host("home.html");
  const { cloneTemplate } = await boot({ hosts: [section] });
  assert.equal(section.children.find((n) => n.tag === "link").removed, false);
  assert.equal(cloneTemplate("row.html").tag, "li", "synchronous once preloaded");
});

test("a failed include shows an error in its host without stopping the others", async () => {
  const original = console.error;
  console.error = () => {};
  try {
    const missing = host("missing.html");
    const fine = host("inner.html");
    await boot({ hosts: [missing, fine] });
    assert.equal("data-include-error" in missing.attrs, true);
    assert.match(missing.textContent, /missing\.html/);
    assert.deepEqual(fine.children.map((n) => n.tag), ["span"]);
  } finally {
    console.error = original;
  }
});

test("after-include scripts are imported in order, and a failing one does not stop the next", async () => {
  const original = console.error;
  console.error = () => {};
  globalThis.__order = [];
  const script = (code) => new El("script", { src: `data:text/javascript,${encodeURIComponent(code)}` });
  try {
    await boot({
      scripts: [
        script("globalThis.__order.push('a')"),
        script("throw new Error('boom')"),
        script("globalThis.__order.push('c')"),
      ],
    });
    await until(() => globalThis.__order.length === 2);
    assert.deepEqual(globalThis.__order, ["a", "c"]);
  } finally {
    console.error = original;
    delete globalThis.__order;
  }
});

test("cloneTemplate refuses a template that was not preloaded", async () => {
  const { cloneTemplate } = await boot();
  assert.throws(() => cloneTemplate("nope.html"), /not preloaded/);
});

test("cloneTemplate gives a fresh element each time and rejects several roots", async () => {
  PAGES["two.html"] = () => [new El("i"), new El("b")];
  const { cloneTemplate } = await withTemplates("row.html", "two.html");
  assert.notEqual(cloneTemplate("row.html"), cloneTemplate("row.html"));
  assert.throws(() => cloneTemplate("two.html"), /exactly one root/);
});

test("cloneTemplate ignores scripts injected next to or inside the root", async () => {
  PAGES["injected.html"] = () => {
    const root = new El("li", {}, [new El("script")]);
    return [root, new El("script")];
  };
  const { cloneTemplate } = await withTemplates("injected.html");
  const root = cloneTemplate("injected.html");
  assert.equal(root.tag, "li");
  assert.equal(root.children.length, 0);
});

test("cloneTemplate sets slot text without touching other nodes", async () => {
  PAGES["card.html"] = () => {
    const card = new El("div");
    const title = new El("b");
    title.dataset.slot = "title";
    const note = new El("small");
    note.dataset.slot = "note";
    note.textContent = "keep";
    card.children = [title, note];
    return [card];
  };
  PAGES["slotted-root.html"] = () => {
    const root = new El("button");
    root.dataset.slot = "label";
    root.children = [new El("span")];
    return [root];
  };
  const { cloneTemplate } = await withTemplates("card.html", "slotted-root.html");

  const card = cloneTemplate("card.html", { title: "<img onerror=x>" });
  assert.equal(card.children[0].textContent, "<img onerror=x>", "plain text, never parsed as markup");
  assert.equal(card.children[1].textContent, "keep", "slots missing from the values are left alone");
  assert.equal(cloneTemplate("card.html").children[0].textContent, "", "no values: left as authored");

  const button = cloneTemplate("slotted-root.html", { label: "Go" });
  assert.equal(button.textContent, "Go", "the root itself can be a slot");
});

test("real template files: whitespace between tags is dropped and slots resolve", async () => {
  globalThis.document = { createElement, querySelectorAll: () => [] };
  await installTemplates(["home/recent-card.html"]);
  const { cloneTemplate } = await import("../src/include.js");
  const card = cloneTemplate("home/recent-card.html", { kind: "Client", verb: "Connect ", endpoint: "h:1" });
  assert.equal(card.tag, "button");
  assert.equal(card.querySelector('[data-slot="endpoint"]').textContent, "h:1");
  assert.equal(card.querySelector('[data-slot="verb"]').textContent, "Connect ");
});

// ── Consistency of the static markup ──

const SRC = new URL("../src/", import.meta.url);
const walkFiles = (dir) =>
  readdirSync(dir).flatMap((name) => {
    const full = new URL(name + (statSync(new URL(name, dir)).isDirectory() ? "/" : ""), dir);
    return name === "assets" ? [] : statSync(full).isDirectory() ? walkFiles(full) : [full];
  });
const rel = (url) => url.href.slice(SRC.href.length);

test("every data-include and x-template points to an existing file", () => {
  const pages = walkFiles(SRC).filter((f) => f.pathname.endsWith(".html"));
  assert.ok(pages.length >= 7, "index.html, five views, close-confirm");
  for (const page of pages) {
    const text = readFileSync(page, "utf8");
    for (const [, url] of text.matchAll(/(?:data-include|href)="([^"]+\.html)"/g)) {
      assert.ok(existsSync(new URL(url, SRC)), `${rel(page)} references ${url}`);
    }
  }
});

test("every template used by a script is preloaded by an x-template link", () => {
  const pages = walkFiles(SRC).filter((f) => f.pathname.endsWith(".html"));
  const preloaded = new Set(
    pages.flatMap((p) => [...readFileSync(p, "utf8").matchAll(/rel="x-template"\s+href="([^"]+)"/g)].map((m) => m[1])),
  );
  const scripts = walkFiles(SRC).filter((f) => f.pathname.endsWith(".js") && !f.pathname.endsWith("include.js") && !f.pathname.includes("oat.min"));
  for (const script of scripts) {
    for (const [, url] of readFileSync(script, "utf8").matchAll(/"([\w-]+\/[\w-]+\.html)"/g)) {
      assert.ok(preloaded.has(url), `${rel(script)} clones ${url} but no x-template link preloads it`);
    }
  }
});

test("index.html hosts every view and only loads include.js as an active script", () => {
  const index = readFileSync(new URL("index.html", SRC), "utf8");
  assert.deepEqual([...index.matchAll(/<script type="module"[^>]*src="([^"]+)"/g)].map((m) => m[1]), ["./include.js"]);
  for (const view of ["home", "session", "templates", "autoreply", "autobuild"]) {
    assert.match(index, new RegExp(`id="view-${view}"[^>]*data-include="${view}/${view}\\.html"`));
  }
  const full = readMarkup();
  assert.doesNotMatch(full, /data-include=/, "the test helper inlines every include");
  assert.match(full, /id="connect-form"/);
});

test("after-include scripts (index + views): config.js first, every view script listed once, files exist", () => {
  const listed = [...readMarkup().matchAll(/<script type="after-include" src="([^"]+)"/g)].map((m) => m[1]);
  assert.equal(listed[0], "config.js");
  assert.equal(new Set(listed).size, listed.length, "no script listed twice");
  const inView = (view) => [...readFileSync(new URL(`${view}/${view}.html`, SRC), "utf8").matchAll(/after-include" src="([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(inView("home"), ["home/connection.js"]);
  assert.deepEqual(inView("session"), ["session/sessions.js", "session/messages.js", "session/session-ui.js", "session/special-chars.js"]);
  assert.deepEqual(inView("templates"), ["templates/templates.js"]);
  assert.deepEqual(inView("autoreply"), ["autoreply/autoreply.js"]);
  assert.deepEqual(inView("autobuild"), ["autobuild/autobuild.js"]);
  for (const common of ["status.js", "nav.js"]) assert.ok(listed.includes(common), common);
  for (const file of listed) assert.ok(existsSync(new URL(file, SRC)), file);
  assert.equal(existsSync(new URL("main.js", SRC)), false);
});
