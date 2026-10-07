import test from "node:test";
import assert from "node:assert/strict";

globalThis.window = { addEventListener() {} };
const panels = ["home", "session", "templates"].map((name) => ({
  dataset: { viewPanel: name },
  classList: { toggle() {} },
}));
globalThis.document = { querySelectorAll: (sel) => (sel === "[data-view-panel]" ? panels : []) };
const { showView, setLeaveGuard, currentView } = await import("../src/nav.js");

test("onActivate runs on direct navigation and only after a deferred guard proceeds", () => {
  const log = [];
  assert.equal(showView("templates", { onActivate: () => log.push("t") }), true);
  assert.equal(currentView(), "templates");

  let proceed;
  setLeaveGuard("templates", (go) => {
    proceed = go;
    return false;
  });
  assert.equal(showView("session", { onActivate: () => log.push("s") }), false);
  assert.equal(currentView(), "templates");
  assert.deepEqual(log, ["t"], "nothing is activated while the guard defers");

  proceed();
  assert.equal(currentView(), "session");
  assert.deepEqual(log, ["t", "s"]);
});

test("a cancelled guard never runs onActivate", () => {
  setLeaveGuard("session", () => false);
  let ran = false;
  assert.equal(showView("home", { onActivate: () => (ran = true) }), false);
  assert.equal(ran, false);
  assert.equal(currentView(), "session");
  setLeaveGuard("session", () => true);
  showView("home", { onActivate: () => (ran = true) });
  assert.equal(ran, true);
});
