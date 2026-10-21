import test from "node:test";
import assert from "node:assert/strict";

import {
  RECENT_KEY,
  MAX_RECENT,
  normalizeEndpoint,
  describeEndpoint,
  endpointText,
  loadRecent,
  recordRecent,
  clearRecent,
} from "../src/home/recent-core.js";

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

test("recent entries of the SSH modes round trip, legacy ones keep loading", () => {
  const ssh = { mode: "ssh-client", host: " H ", port: 2575, sshHost: " VM ", sshPort: "22", sshUser: " lis " };
  assert.deepEqual(normalizeEndpoint(ssh), {
    mode: "ssh-client", host: "h", port: 2575, sshHost: "vm", sshPort: 22, sshUser: "lis",
  });
  assert.deepEqual(normalizeEndpoint({ mode: "ssh-server", host: "x", port: 5, sshHost: "vm", sshPort: 2222 }), {
    mode: "ssh-server", host: "", port: 5, sshHost: "vm", sshPort: 2222, sshUser: "",
  });
  assert.equal(normalizeEndpoint({ ...ssh, sshHost: "" }), null);
  assert.equal(normalizeEndpoint({ ...ssh, sshHost: "a b" }), null);
  assert.equal(normalizeEndpoint({ ...ssh, sshPort: 0 }), null);
  assert.equal(normalizeEndpoint({ ...ssh, sshPort: undefined }), null);
  assert.equal(normalizeEndpoint({ mode: "ssh-server", port: 5 }), null);

  const storage = memoryStorage({
    [RECENT_KEY]: JSON.stringify([
      { mode: "client", host: "legacy", port: 1, lastUsed: 1 },
      { mode: "server", host: "", port: 2, lastUsed: 2 },
    ]),
  });
  recordRecent(storage, ssh, 10);
  recordRecent(storage, { ...ssh, sshUser: "" }, 20); // other tunnel user = other entry
  recordRecent(storage, { ...ssh, sshPort: 2222 }, 30);
  const { entries } = recordRecent(storage, { ...ssh, sshUser: "lis" }, 40); // same as the first
  assert.deepEqual(
    entries.map((e) => [e.mode, e.host, e.port, e.sshHost, e.sshPort, e.sshUser, e.lastUsed]),
    [
      ["ssh-client", "h", 2575, "vm", 22, "lis", 40],
      ["ssh-client", "h", 2575, "vm", 2222, "lis", 30],
      ["ssh-client", "h", 2575, "vm", 22, "", 20],
      ["server", "", 2, undefined, undefined, undefined, 2],
      ["client", "legacy", 1, undefined, undefined, undefined, 1],
    ],
  );
  assert.deepEqual(loadRecent(storage).entries, entries);
});

test("describeEndpoint / endpointText: compact labels for the four modes", () => {
  const ep = (o) => normalizeEndpoint(o);
  const tunnel = { sshHost: "vm", sshPort: 22, sshUser: "lis" };
  assert.equal(describeEndpoint(ep({ mode: "client", host: "h", port: 1 })), "h:1");
  assert.equal(describeEndpoint(ep({ mode: "server", port: 2 })), "Server :2");
  assert.equal(describeEndpoint(ep({ mode: "ssh-client", host: "h", port: 1, ...tunnel })), "lis@vm:22 → h:1");
  assert.equal(describeEndpoint(ep({ mode: "ssh-client", host: "h", port: 1, ...tunnel, sshUser: "" })), "vm:22 → h:1");
  assert.equal(describeEndpoint(ep({ mode: "ssh-server", port: 2, ...tunnel })), "Server :2 ← lis@vm:22");
  assert.equal(endpointText(ep({ mode: "ssh-server", port: 2, ...tunnel })), ":2 ← lis@vm:22");
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
