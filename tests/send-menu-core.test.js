// Pure helpers of the template split Send button (labels, menu items, keyboard moves, placement).
import test from "node:test";
import assert from "node:assert/strict";
import { mainLabel, mainHint, menuItems, hasSendable, itemText, moveFocus, initialFocus, placeMenu } from "../src/templates/send-menu-core.js";

const sessionsList = [
  { id: "a", label: "Alpha", status: "connected", op: null },
  { id: "b", label: "Beta", status: "connecting", op: null },
  { id: "c", label: "Gamma", status: "connected", op: "closing" },
  { id: "d", label: "Delta", status: "connected", op: null },
];
// Stand-in for sessions.sendTarget: connected and idle only.
const sendTarget = (id) => {
  const s = sessionsList.find((x) => x.id === id);
  return s && s.status === "connected" && !s.op ? { id } : null;
};

test("main label follows the selected session and falls back to Send", () => {
  assert.equal(mainLabel({ label: "Lab" }), "Send to Lab");
  assert.equal(mainLabel(null), "Send");
  assert.match(mainHint({ label: "Lab" }, true), /Lab/);
  assert.match(mainHint({ label: "Lab" }, false), /Connect/);
  assert.match(mainHint(null, false), /Select/);
});

test("menu items carry name, state, current mark and the send rule's verdict", () => {
  const items = menuItems(sessionsList, "b", sendTarget);
  assert.deepEqual(items.map((i) => i.enabled), [true, false, false, true]);
  assert.deepEqual(items.map((i) => i.current), [false, true, false, false]);
  assert.deepEqual(items.map((i) => i.state), ["Connected", "Connecting", "Closing", "Connected"]);
  assert.equal(itemText(items[1]), "Beta (current): Connecting");
  assert.equal(hasSendable(items), true, "usable although the current session cannot send");
  assert.equal(hasSendable(menuItems([sessionsList[1]], "b", sendTarget)), false);
  assert.equal(hasSendable([]), false);
});

test("keyboard moves skip disabled entries and wrap", () => {
  const items = menuItems(sessionsList, "b", sendTarget); // enabled: 0 and 3
  assert.equal(moveFocus(items, -1, "ArrowDown"), 0);
  assert.equal(moveFocus(items, 0, "ArrowDown"), 3);
  assert.equal(moveFocus(items, 3, "ArrowDown"), 0);
  assert.equal(moveFocus(items, 0, "ArrowUp"), 3);
  assert.equal(moveFocus(items, -1, "ArrowUp"), 3);
  assert.equal(moveFocus(items, 3, "Home"), 0);
  assert.equal(moveFocus(items, 0, "End"), 3);
  assert.equal(moveFocus(menuItems([sessionsList[1]], "b", sendTarget), -1, "ArrowDown"), -1);
  assert.equal(initialFocus(items), 0, "current is disabled: first enabled");
  assert.equal(initialFocus(menuItems(sessionsList, "d", sendTarget)), 3, "current enabled session first");
});

test("placement stays inside the window: below when it fits, else above, clamped", () => {
  const viewport = { width: 800, height: 600 };
  const size = { width: 200, height: 150 };
  const below = placeMenu({ top: 100, bottom: 130, right: 500, left: 450 }, size, viewport);
  assert.equal(below.up, false);
  assert.equal(below.top, 134);
  assert.equal(below.left, 300, "right-aligned with the arrow");
  const above = placeMenu({ top: 540, bottom: 570, right: 500, left: 450 }, size, viewport);
  assert.equal(above.up, true);
  assert.equal(above.top, 540 - 4 - 150);
  const edge = placeMenu({ top: 540, bottom: 570, right: 60, left: 20 }, size, viewport);
  assert.equal(edge.left, 8, "clamped to the left margin");
  const tall = placeMenu({ top: 540, bottom: 570, right: 500, left: 450 }, { width: 200, height: 900 }, viewport);
  assert.ok(tall.top >= 8 && tall.maxHeight <= 600, "tall menus scroll instead of overflowing");
  assert.equal(tall.top, 540 - 4 - tall.maxHeight);
});
