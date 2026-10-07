import test from "node:test";
import assert from "node:assert/strict";

import {
  RECENT_KEY,
  MAX_RECENT,
  normalizeEndpoint,
  loadRecent,
  recordRecent,
  clearRecent,
} from "../src/recent-core.js";

function memoryStorage(initial = {}) {
  const data = { ...initial };
  return {
    data,
    getItem: (k) => (k in data ? data[k] : null),
    setItem: (k, v) => {
      data[k] = v;
    },
    removeItem: (k) => {
      delete data[k];
    },
  };
}

test("normalizeEndpoint trims/lowercases hosts and drops the host of servers", () => {
  assert.deepEqual(normalizeEndpoint({ mode: "client", host: "  LIS-Pre.Local ", port: "2575" }), {
    mode: "client",
    host: "lis-pre.local",
    port: 2575,
  });
  assert.deepEqual(normalizeEndpoint({ mode: "server", host: "ignored", port: 10068 }), {
    mode: "server",
    host: "",
    port: 10068,
  });
  assert.equal(normalizeEndpoint({ mode: "client", host: "", port: 1 }), null);
  assert.equal(normalizeEndpoint({ mode: "client", host: "a b", port: 1 }), null);
  assert.equal(normalizeEndpoint({ mode: "client", host: "a", port: 0 }), null);
  assert.equal(normalizeEndpoint({ mode: "client", host: "a", port: 70000 }), null);
  assert.equal(normalizeEndpoint({ mode: "client", host: "a", port: 1.5 }), null);
  assert.equal(normalizeEndpoint({ mode: "other", host: "a", port: 1 }), null);
});

test("recordRecent dedupes by mode+host+port and keeps most recent first", () => {
  const storage = memoryStorage();
  recordRecent(storage, { mode: "client", host: "Host", port: 1 }, 100);
  recordRecent(storage, { mode: "client", host: "other", port: 1 }, 200);
  recordRecent(storage, { mode: "server", host: "", port: 1 }, 300);
  const { entries } = recordRecent(storage, { mode: "client", host: " HOST ", port: "1" }, 400);
  assert.deepEqual(
    entries.map((e) => [e.mode, e.host, e.port, e.lastUsed]),
    [
      ["client", "host", 1, 400],
      ["server", "", 1, 300],
      ["client", "other", 1, 200],
    ],
  );
  assert.deepEqual(loadRecent(storage).entries, entries);
});

test("recordRecent bounds the list", () => {
  const storage = memoryStorage();
  for (let i = 1; i <= MAX_RECENT + 5; i++) {
    recordRecent(storage, { mode: "client", host: "h", port: i }, i);
  }
  const { entries } = loadRecent(storage);
  assert.equal(entries.length, MAX_RECENT);
  assert.equal(entries[0].port, MAX_RECENT + 5);
  assert.equal(entries.at(-1).port, 6);
});

test("corrupt storage yields an empty list and is replaced on next record", () => {
  const storage = memoryStorage({ [RECENT_KEY]: "{oops" });
  const loaded = loadRecent(storage);
  assert.deepEqual(loaded.entries, []);
  assert.ok(loaded.error);

  const notList = memoryStorage({ [RECENT_KEY]: '{"a":1}' });
  assert.ok(loadRecent(notList).error);

  const { entries, error } = recordRecent(storage, { mode: "server", host: "", port: 5 }, 1);
  assert.equal(error, null);
  assert.equal(entries.length, 1);
});

test("invalid stored records are dropped, valid ones kept", () => {
  const storage = memoryStorage({
    [RECENT_KEY]: JSON.stringify([
      { mode: "client", host: "ok", port: 1, lastUsed: 5 },
      { mode: "client", host: "", port: 1, lastUsed: 4 },
      null,
      "x",
      { mode: "client", host: "OK", port: 1, lastUsed: 3 },
    ]),
  });
  const { entries, error } = loadRecent(storage);
  assert.equal(error, null);
  assert.equal(entries.length, 1);
});

test("storage failures are reported, not thrown", () => {
  const failing = {
    getItem: () => null,
    setItem: () => {
      throw new Error("quota");
    },
    removeItem: () => {
      throw new Error("denied");
    },
  };
  assert.ok(recordRecent(failing, { mode: "server", host: "", port: 1 }).error);
  assert.ok(clearRecent(failing));
  assert.ok(recordRecent(memoryStorage(), { mode: "client", host: "", port: 1 }).error);
});
