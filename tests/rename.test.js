import test from "node:test";
import assert from "node:assert/strict";
import { createSessionController } from "../src/session/sessions-controller.js";
import { createSessionStore } from "../src/session/sessions-core.js";
import {
  NAMES_KEY,
  RECENT_KEY,
  cleanName,
  connectionLabel,
  loadNames,
  loadRecent,
  recordRecent,
  saveName,
  savedName,
} from "../src/home/recent-core.js";

function memoryStorage(initial = {}) {
  const data = { ...initial };
  return {
    data,
    failing: false,
    getItem: (k) => (k in data ? data[k] : null),
    setItem(k, v) {
      if (this.failing) throw new Error("quota");
      data[k] = v;
    },
    removeItem: (k) => delete data[k],
  };
}

const lis = { mode: "client", host: "lis.local", port: 2575 };
const other = { mode: "client", host: "other.local", port: 2575 };
const req = { type: "ClientConnectRequest", ip: "h", port: 1 };

test("cleanName trims, collapses blanks and caps the length", () => {
  assert.equal(cleanName("  My   LIS \n"), "My LIS");
  assert.equal(cleanName("   "), "");
  assert.equal(cleanName(null), "");
  assert.equal(cleanName("x".repeat(100)).length, 60);
});

test("a saved name is found by connection details, whatever the host case", () => {
  const storage = memoryStorage();
  assert.deepEqual(saveName(storage, lis, " Lab LIS "), { name: "Lab LIS", error: null });
  const { names } = loadNames(storage);
  assert.equal(savedName(names, { ...lis, host: "LIS.local", port: "2575" }), "Lab LIS");
  assert.equal(savedName(names, other), "");
  assert.equal(connectionLabel(names, lis), "Lab LIS");
  assert.equal(connectionLabel(names, other), "other.local:2575");
});

test("an empty name clears the saved name", () => {
  const storage = memoryStorage();
  saveName(storage, lis, "Lab");
  saveName(storage, other, "Other");
  assert.equal(saveName(storage, lis, "  ").name, "");
  const { names } = loadNames(storage);
  assert.equal(savedName(names, lis), "");
  assert.equal(savedName(names, other), "Other");
});

test("names are kept apart from the recent list: no entry added, order kept, names survive eviction and reconnects", () => {
  const storage = memoryStorage();
  recordRecent(storage, lis, 1);
  recordRecent(storage, other, 2);
  const before = storage.data[RECENT_KEY];

  saveName(storage, lis, "Lab");
  assert.equal(storage.data[RECENT_KEY], before, "renaming does not touch the recent list");
  saveName(storage, { mode: "server", host: "", port: 9000 }, "Not recent");
  assert.equal(loadRecent(storage).entries.length, 2);

  recordRecent(storage, lis, 3); // reconnect moves it first
  assert.deepEqual(loadRecent(storage).entries.map((e) => e.host), ["lis.local", "other.local"]);
  for (let i = 0; i < 20; i += 1) recordRecent(storage, { mode: "client", host: `h${i}`, port: 1 }, 10 + i);
  assert.equal(loadRecent(storage).entries.some((e) => e.host === "lis.local"), false, "evicted");
  const { names } = loadNames(storage);
  assert.equal(savedName(names, lis), "Lab");
  assert.equal(savedName(names, { mode: "server", port: 9000 }), "Not recent");
});

test("old recent entries without a name stay valid and corrupt names are ignored", () => {
  const storage = memoryStorage({
    [RECENT_KEY]: JSON.stringify([{ ...lis, lastUsed: 1 }]),
    [NAMES_KEY]: "{not json",
  });
  assert.equal(loadRecent(storage).entries.length, 1);
  const loaded = loadNames(storage);
  assert.equal(loaded.names.size, 0);
  assert.match(loaded.error, /unreadable/);
});

test("a failing save reports the error and saves nothing", () => {
  const storage = memoryStorage();
  storage.failing = true;
  const result = saveName(storage, lis, "Lab");
  assert.match(result.error, /Cannot save the connection name/);
  assert.equal(NAMES_KEY in storage.data, false);
});

function setup(storage = memoryStorage()) {
  let n = 0;
  const store = createSessionStore({ newId: () => `s${++n}`, onListenerError: (e) => { throw e; } });
  const controller = createSessionController({ invoke: async () => {}, storage, log: {}, store });
  return { controller, store, storage };
}

test("renaming relabels every linked session, by details and not by name, and nothing else", async () => {
  const { controller, store, storage } = setup();
  const a = controller.start({ mode: "client", label: "x", endpoint: lis, req });
  const b = controller.start({ mode: "client", label: "y", endpoint: { ...lis }, req });
  const c = controller.start({ mode: "client", label: "x", endpoint: other, req });
  await Promise.all([a.done, b.done, c.done]);
  store.setDraft(a.id, "draft");
  store.addLocal(a.id, "systemerror", "kept");
  const events = [];
  store.subscribe((e) => events.push(e.type));

  assert.deepEqual(controller.renameConnection(lis, "  Lab  "), { ok: true, name: "Lab" });
  assert.deepEqual([a.id, b.id, c.id].map((id) => store.get(id).label), ["Lab", "Lab", "x"]);
  assert.equal(store.get(a.id).draft, "draft");
  assert.equal(store.get(a.id).records.length, 1);
  assert.deepEqual(store.get(a.id).endpoint, lis);
  assert.ok(events.includes("renamed"));
  assert.equal(savedName(loadNames(storage).names, lis), "Lab");

  assert.equal(controller.renameConnection(lis, "").name, "");
  assert.deepEqual([a.id, b.id, c.id].map((id) => store.get(id).label), ["lis.local:2575", "lis.local:2575", "x"]);
});

test("a failed save leaves session names unchanged and reports the error", async () => {
  const { controller, store, storage } = setup();
  const a = controller.start({ mode: "client", label: "lis.local:2575", endpoint: lis, req });
  await a.done;
  storage.failing = true;
  const result = controller.renameConnection(lis, "Lab");
  assert.equal(result.ok, false);
  assert.match(result.error, /quota/);
  assert.equal(store.get(a.id).label, "lis.local:2575");
});

test("close prompt and send targets use the shared name", async () => {
  const { controller, store } = setup();
  const a = controller.start({ mode: "client", label: "x", endpoint: lis, req });
  await a.done;
  controller.renameConnection(lis, "Lab");
  assert.equal(controller.closeImpact(a.id).label, "Lab");
  store.applyStatus({ session_id: a.id, attempt: store.get(a.id).attempt, status: "connected" });
  assert.equal(controller.sendTarget(a.id).label, "Lab");
});
