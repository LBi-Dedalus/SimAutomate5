// Test helpers for the data-include / template markup. Not a test file: the runner glob
// (tests/*.test.js) does not pick it up.
import { readFileSync } from "node:fs";

const SRC = new URL("../../src/", import.meta.url);
const readSrc = (path) => readFileSync(new URL(path, SRC), "utf8");

/** src/index.html with every data-include file inlined, as the browser ends up with after startup. */
export function readMarkup(path = "index.html") {
  return readSrc(path).replace(
    /<(\w+)([^>]*?)\s+data-include="([^"]+)"([^>]*)><\/\1>/g,
    (_, tag, before, url, after) => `<${tag}${before}${after}>${readMarkup(url)}</${tag}>`,
  );
}

function camel(name) {
  return name.replace(/-(\w)/g, (_, c) => c.toUpperCase());
}

/** Minimal parser for template files: elements, attributes and text-only leaves. */
function parseTemplate(html, create) {
  const roots = [];
  const stack = [];
  const token = /<!--[\s\S]*?-->|<\/(\w+)>|<(\w+)((?:\s+[^\s=>/]+(?:="[^"]*")?)*)\s*\/?>|([^<]+)/g;
  for (const [all, close, tag, attrs, text] of html.matchAll(token)) {
    if (all.startsWith("<!--")) continue;
    if (close) {
      stack.pop();
    } else if (tag) {
      const node = create(tag);
      node.dataset ??= {};
      for (const [, name, value = ""] of (attrs ?? "").matchAll(/([^\s=]+)(?:="([^"]*)")?/g)) {
        if (name === "class") node.className = value;
        else if (name.startsWith("data-")) node.dataset[camel(name.slice(5))] = value;
        else {
          node.setAttribute?.(name, value);
          node[name] = value;
        }
      }
      const parent = stack.at(-1);
      node.parent = parent;
      (parent?.children ?? roots).push(node);
      stack.push(node);
      if (/\/>$/.test(all)) stack.pop();
    } else if (text.trim()) {
      stack.at(-1).textContent = text;
    }
  }
  return roots;
}

/**
 * Makes the fake `document` able to parse template files and has include.js preload them, reading
 * the real files from src/. Call it after `globalThis.document` is set and before anything imports
 * include.js (its startup runs once, at the first import).
 */
export async function installTemplates(urls) {
  const create = document.createElement.bind(document);
  // The fake nodes only need to understand the [data-slot="x"],[data-slot="y"] selectors cloneTemplate uses.
  const proto = Object.getPrototypeOf(create("div"));
  proto.querySelectorAll ??= function (selector) {
    const names = selector.split(",").map((s) => s.slice('[data-slot="'.length, -2));
    const found = [];
    const visit = (node) => {
      for (const child of node.children) {
        if (child.dataset?.slot !== undefined && names.includes(child.dataset.slot)) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  };
  proto.querySelector ??= function (selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  };
  proto.remove ??= function () {
    if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this);
  };
  document.createElement = (tag) =>
    tag === "template"
      ? {
          set innerHTML(html) {
            const roots = parseTemplate(html, create);
            // Clones are parsed again: fake nodes are not copyable, but a parse always gives a fresh tree.
            for (const root of roots) root.cloneNode = () => parseTemplate(html, create)[0];
            this.content = { children: roots, querySelectorAll: () => [] };
          },
        }
      : create(tag);
  globalThis.fetch = async (url) => ({ ok: true, status: 200, text: async () => (url === "preload.html" ? "" : readSrc(url)) });

  // One included file whose x-template links are the requested templates. include.js asks for the
  // after-include scripts once every include and preload is done, which is how the test knows.
  let finished;
  const done = new Promise((resolve) => (finished = resolve));
  const links = urls.map((href) => ({ getAttribute: () => href }));
  const host = {
    getAttribute: () => "preload.html",
    setAttribute() {},
    removeAttribute() {},
    replaceChildren() {},
    querySelectorAll: (selector) => (selector.startsWith("link") ? links : []),
  };
  const originalQuery = document.querySelectorAll;
  document.querySelectorAll = (selector) => {
    if (selector === "[data-include]") return [host];
    if (selector === 'script[type="after-include"]') finished();
    return [];
  };
  await import("../../src/include.js");
  await done;
  document.querySelectorAll = originalQuery;
}
