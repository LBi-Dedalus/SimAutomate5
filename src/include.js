// Declarative HTML includes and file-per-template cloning. No build step: everything is fetched
// at startup, relative to the page (src/index.html).
//
//   <section data-include="home/home.html"></section>
//       the element is filled with the markup of that file at startup (nested includes work).
//
//   <link rel="x-template" href="home/recent-card.html" />   (inside an included file)
//       preloads that file so cloneTemplate() can stay synchronous.
//
//   cloneTemplate("home/recent-card.html", { endpoint: "10.0.0.1:6000" });
//       a fresh root element each call, with the text of [data-slot="endpoint"] set.
//
// A template file contains exactly one root element (checked once, when it is preloaded; scripts
// injected by dev servers are ignored), and the whitespace between its tags is dropped (put text
// in slots). The parsed root is cached and cloned on every call. Values always go through textContent, never through innerHTML,
// so protocol text cannot inject markup.
//
// Event listeners are declared in the markup too:
//   <button data-action="click:ar.remove keydown:ar.key">
//       a space-separated list of event:handler pairs. The handler is looked up in window.actions
//       when the event fires (window.actions.ar.remove, called as handler(event, element)), so views
//       register it with `window.actions.ar = { remove, key }` in any order. An empty handler
//       ("click:") only stops the event from reaching the ancestors, e.g. a checkbox in a clickable row.
// The static markup is bound once the includes are loaded (before the scripts run), and every
// cloneTemplate() result is bound before it is returned. Malformed attributes in a template file
// fail at preload.
//
// Startup (index.html loads this file, which runs once the page is parsed): the includes are
// fetched first, then every
//   <script type="after-include" src="view/file.js"></script>
// found in the page or in an included file is imported in document order (paths are relative
// to index.html). Those scripts run their initialisation at import time, so they need no
// DOMContentLoaded listener, and config.js (listed first) hydrates the fields before the others.
import { logError } from "./log.js";

const sources = new Map(); // url -> Promise<string>
const templates = new Map(); // url -> validated root element, once preloaded

if (typeof window !== "undefined") window.actions ??= {};

/** Splits a data-action value ("click:ar.select keydown:ar.key") into {type, name} pairs. */
function parseActions(value) {
  return value
    .trim()
    .split(/\s+/)
    .map((token) => {
      const match = /^([a-z]+):([\w.]*)$/i.exec(token);
      if (!match) throw new Error(`Invalid data-action "${token}" (expected event:handler)`);
      return { type: match[1], name: match[2] };
    });
}

/** Calls window.actions.<name>(event, element); a missing handler or a failure is logged. */
function runAction(name, event, element) {
  const report = (error) => logError(`Action ${name}: ${error?.message ?? error}`, "include.js:runAction");
  const handler = name.split(".").reduce((scope, key) => scope?.[key], window.actions);
  if (typeof handler !== "function") return report("no such handler");
  try {
    Promise.resolve(handler(event, element)).catch(report);
  } catch (error) {
    report(error);
  }
}

/** Adds the listeners declared by the [data-action] elements of root (root included). */
function bindActions(root) {
  const nodes = [...root.querySelectorAll("[data-action]")];
  if (root.dataset?.action !== undefined) nodes.unshift(root);
  for (const node of nodes) {
    for (const { type, name } of parseActions(node.dataset.action)) {
      node.addEventListener(type, (event) => (name ? runAction(name, event, node) : event.stopPropagation()));
    }
  }
}

function fetchText(url) {
  if (!sources.has(url)) {
    const request = fetch(url).then((response) => {
      if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
      return response.text();
    });
    sources.set(url, request);
    request.catch(() => sources.delete(url));
  }
  return sources.get(url);
}

function parse(html) {
  const holder = document.createElement("template");
  holder.innerHTML = html;
  return holder.content;
}

/** Fetches and injects the file named by every [data-include] under root. Failures are logged and shown in the host. */
async function loadIncludes(root = document) {
  const hosts = [...root.querySelectorAll("[data-include]")];
  await Promise.all(
    hosts.map(async (host) => {
      const url = host.getAttribute("data-include");
      try {
        host.replaceChildren(parse(await fetchText(url)));
        host.removeAttribute("data-include");
        const links = [...host.querySelectorAll('link[rel="x-template"]')];
        await preloadTemplates(links.map((link) => link.getAttribute("href")));
        await loadIncludes(host);
      } catch (error) {
        host.setAttribute("data-include-error", "");
        host.textContent = `Could not load ${url}.`;
        logError(String(error?.message ?? error), "include.js:loadIncludes");
        console.error(error);
      }
    }),
  );
}

/** Parses a template file once and returns its single root element, or throws if it has none or several. */
function parseTemplate(url, html) {
  const content = parse(html.replace(/>\s+</g, "><").trim());
  // Dev servers may inject scripts (e.g. hot reload) into served HTML; they are not part of the template.
  content.querySelectorAll("script").forEach((script) => script.remove());
  if (content.children.length !== 1) {
    const found = [...content.children].map((node) => node.localName ?? node.tag).join(", ") || "none";
    throw new Error(`Template ${url} must contain exactly one root element (found: ${found})`);
  }
  const root = content.children[0];
  try {
    for (const node of [root, ...root.querySelectorAll("[data-action]")]) {
      if (node.dataset?.action !== undefined) parseActions(node.dataset.action);
    }
  } catch (error) {
    throw new Error(`Template ${url}: ${error.message}`);
  }
  return root;
}

/** Fetches, parses and validates template files ahead of time so cloneTemplate() can be called synchronously. */
async function preloadTemplates(urls) {
  await Promise.all(
    urls.map(async (url) => {
      if (!templates.has(url)) templates.set(url, parseTemplate(url, await fetchText(url)));
    }),
  );
}

const slotSelector = (name) => `[data-slot="${name.replace(/["\\]/g, "\\$&")}"]`;

/** Sets the text of each [data-slot] whose name is in values. */
function fill(root, values) {
  const self = root.dataset?.slot;
  if (self !== undefined && self in values) {
    // Setting the root's text replaces its children, so there is nothing left to fill below it.
    root.textContent = values[self];
    return;
  }
  const names = Object.keys(values);
  if (names.length === 0) return;
  for (const node of root.querySelectorAll(names.map(slotSelector).join(","))) {
    node.textContent = values[node.dataset.slot];
  }
}

/**
 * Returns a new element built from the template file (it must have been preloaded), with the
 * text of each [data-slot] named in values set and its [data-action] listeners attached.
 */
export function cloneTemplate(url, values = {}) {
  const template = templates.get(url);
  if (template === undefined) throw new Error(`Template not preloaded: ${url}`);
  const root = template.cloneNode(true);
  fill(root, values);
  bindActions(root);
  return root;
}

async function start() {
  await loadIncludes();
  try {
    bindActions(document);
  } catch (error) {
    logError(`Could not bind actions: ${error?.message ?? error}`, "include.js:start");
    console.error(error);
  }
  // Collected once the views are in the DOM, so the scripts named by their markup are included.
  const scripts = [...document.querySelectorAll('script[type="after-include"]')];
  for (const script of scripts) {
    const url = new URL(script.getAttribute("src"), document.baseURI).href;
    // A script that fails to load or initialise must not stop the ones after it.
    try {
      await import(url);
    } catch (error) {
      logError(`Could not start ${url}: ${error?.message ?? error}`, "include.js:start");
      console.error(error);
    }
  }
}
start();
