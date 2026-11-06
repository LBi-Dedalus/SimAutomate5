import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { readMarkup } from "./helpers/markup.js";

const { showAppVersion, VERSION_UNAVAILABLE } = await import("../src/app-version.js");

test("the label shows the version read at runtime", async () => {
  const element = { textContent: "" };
  await showAppVersion({ getVersion: async () => "9.8.7", element, reportError: () => assert.fail() });
  assert.equal(element.textContent, "SimAutomate v9.8.7");
});

test("a failed read shows 'Version unavailable' and reports the error", async () => {
  const element = { textContent: "" };
  const reported = [];
  await showAppVersion({
    getVersion: async () => {
      throw new Error("denied");
    },
    element,
    reportError: (message) => reported.push(message),
  });
  assert.equal(element.textContent, VERSION_UNAVAILABLE);
  assert.equal(reported.length, 1);
  assert.match(reported[0], /denied/);
});

test("the label is written once: a session change has nothing that rewrites it", () => {
  const ui = readFileSync(new URL("../src/session/session-ui.js", import.meta.url), "utf8");
  assert.doesNotMatch(ui, /app-version|side-endpoint/);
});

test("the sidebar footer holds only the version label; status pills stay in the session view", () => {
  const html = readMarkup();
  const footer = html.match(/<aside id="sidebar">[\s\S]*?<footer>([\s\S]*?)<\/footer>/)[1];
  assert.match(footer, /id="app-version"/);
  assert.doesNotMatch(footer, /data-status-pill|id="status"|side-endpoint/);
  assert.doesNotMatch(html, /side-endpoint/);
  assert.match(html, /data-status-pill/, "the Session view keeps its connection indicator");
});

test("the default capability allows reading the app version", () => {
  const caps = JSON.parse(readFileSync(new URL("../src-tauri/capabilities/default.json", import.meta.url), "utf8"));
  assert.ok(caps.permissions.includes("core:default") || caps.permissions.includes("core:app:allow-version"));
});
