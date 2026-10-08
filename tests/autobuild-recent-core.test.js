import test from "node:test";
import assert from "node:assert/strict";
import {
  RECENT_BUILDS_KEY,
  MAX_RECENT_BUILDS,
  detectKind,
  loadRecentBuilds,
  recordBuild,
  deleteBuild,
  clearRecentBuilds,
  formatShortTime,
  previewInput,
} from "../src/autobuild-recent-core.js";

function memoryStorage(initial = {}, { limit = Infinity } = {}) {
  const data = new Map(Object.entries(initial));
  return {
    data,
    getItem: (k) => (data.has(k) ? data.get(k) : null),
    setItem(k, v) {
      if (v.length > limit) throw new Error("QuotaExceededError");
      data.set(k, v);
    },
    removeItem: (k) => data.delete(k),
  };
}

const build = (input, extra = {}) => ({ input, output: `out:${input}`, noEtb: false, ...extra });

test("key and kind detection mirror the backend", () => {
  assert.equal(RECENT_BUILDS_KEY, "simautomate:recent-autobuilds");
  assert.equal(detectKind("H|\\^&|||x"), "ASTM");
  assert.equal(detectKind("  \n MSH|^~\\&|a"), "HL7");
  assert.equal(detectKind("hello"), "Raw");
  assert.equal(detectKind("MSH"), "Raw");
  assert.equal(detectKind(undefined), "Raw");
});

test("record stores newest first with a unique id and derived kind", () => {
  const s = memoryStorage();
  recordBuild(s, build("H|a"), 1000);
  const { entries, error, entry } = recordBuild(s, build("MSH|b", { noEtb: true }), 2000);
  assert.equal(error, null);
  assert.deepEqual(entries.map((e) => e.input), ["MSH|b", "H|a"]);
  assert.equal(entry.kind, "HL7");
  assert.equal(entries[1].kind, "ASTM");
  assert.equal(entries[0].noEtb, true);
  assert.notEqual(entries[0].id, entries[1].id);
  assert.equal(typeof entries[0].id, "string");
  assert.deepEqual(loadRecentBuilds(s).entries, entries);
});

test("empty input is not recorded", () => {
  const s = memoryStorage();
  for (const input of ["", "   \n", undefined, 5]) {
    const r = recordBuild(s, build(input), 1);
    assert.equal(r.entry, null);
    assert.equal(r.error, null);
  }
  assert.equal(s.data.has(RECENT_BUILDS_KEY), false);
});

test("same input + noEtb is deduplicated: moved to top, output and time refreshed, id kept", () => {
  const s = memoryStorage();
  const first = recordBuild(s, build("A"), 1).entry;
  recordBuild(s, build("B"), 2);
  recordBuild(s, build("A", { noEtb: true }), 3); // different flag => distinct entry
  const { entries } = recordBuild(s, { input: "A", output: "new", noEtb: false }, 4);
  assert.deepEqual(entries.map((e) => [e.input, e.noEtb]), [["A", false], ["A", true], ["B", false]]);
  assert.equal(entries[0].output, "new");
  assert.equal(entries[0].lastUsed, 4);
  assert.equal(entries[0].id, first.id);
});

test("list is capped at 20, oldest dropped", () => {
  const s = memoryStorage();
  for (let i = 0; i < MAX_RECENT_BUILDS + 5; i++) recordBuild(s, build(`m${i}`), i + 1);
  const { entries } = loadRecentBuilds(s);
  assert.equal(entries.length, 20);
  assert.equal(entries[0].input, "m24");
  assert.equal(entries[19].input, "m5");
});

test("load sanitizes entries: bad shapes, empty input, duplicates, ids, kind, flags", () => {
  const raw = JSON.stringify([
    null,
    "str",
    { input: 5 },
    { input: "   " },
    { input: "H|x", output: 3, noEtb: "yes", kind: "HL7", lastUsed: "no", id: "" },
    { input: "MSH|y", output: "o", noEtb: true, lastUsed: 50, id: "dup" },
    { input: "MSH|z", output: "o", lastUsed: 40, id: "dup" },
    { input: "MSH|y", output: "older", noEtb: true, lastUsed: 10, id: "other" },
  ]);
  const { entries, error } = loadRecentBuilds(memoryStorage({ [RECENT_BUILDS_KEY]: raw }));
  assert.equal(error, null);
  assert.deepEqual(entries.map((e) => e.input), ["MSH|y", "MSH|z", "H|x"]);
  assert.equal(new Set(entries.map((e) => e.id)).size, 3);
  const h = entries[2];
  assert.equal(h.kind, "ASTM");
  assert.equal(h.output, "");
  assert.equal(h.noEtb, false);
  assert.equal(h.lastUsed, 0);
  assert.equal(entries[0].output, "o");
});

test("corrupt, non-array and throwing storage give an empty list and an error", () => {
  const corrupt = loadRecentBuilds(memoryStorage({ [RECENT_BUILDS_KEY]: "{oops" }));
  assert.deepEqual(corrupt.entries, []);
  assert.match(corrupt.error, /unreadable/);
  const obj = loadRecentBuilds(memoryStorage({ [RECENT_BUILDS_KEY]: '{"a":1}' }));
  assert.deepEqual(obj.entries, []);
  assert.match(obj.error, /not a list/);
  const broken = loadRecentBuilds({
    getItem() {
      throw new Error("denied");
    },
  });
  assert.deepEqual(broken.entries, []);
  assert.match(broken.error, /denied/);
  assert.deepEqual(loadRecentBuilds(memoryStorage()), { entries: [], error: null });
});

test("record on corrupt storage starts a fresh list", () => {
  const s = memoryStorage({ [RECENT_BUILDS_KEY]: "garbage" });
  const r = recordBuild(s, build("A"), 1);
  assert.equal(r.error, null);
  assert.deepEqual(loadRecentBuilds(s).entries.map((e) => e.input), ["A"]);
});

test("quota failure drops the oldest entries until it fits", () => {
  const s = memoryStorage();
  for (let i = 0; i < 6; i++) recordBuild(s, build(`m${i}`), i + 1);
  const size = s.data.get(RECENT_BUILDS_KEY).length;
  const small = memoryStorage({ [RECENT_BUILDS_KEY]: s.data.get(RECENT_BUILDS_KEY) }, { limit: Math.floor(size * 0.6) });
  const r = recordBuild(small, build("new"), 100);
  assert.equal(r.error, null);
  assert.equal(r.entries[0].input, "new");
  assert.ok(r.entries.length < 7 && r.entries.length >= 1);
  assert.deepEqual(loadRecentBuilds(small).entries, r.entries);
});

test("quota failure that cannot be fixed reports an error and never throws", () => {
  const s = memoryStorage({}, { limit: 0 });
  const r = recordBuild(s, build("A"), 1);
  assert.match(r.error, /Cannot save recent autobuilds/);
  const throwing = {
    getItem: () => null,
    setItem() {
      throw new Error("blocked");
    },
  };
  assert.match(recordBuild(throwing, build("A"), 1).error, /blocked/);
});

test("delete one and clear all", () => {
  const s = memoryStorage();
  const a = recordBuild(s, build("A"), 1).entry;
  recordBuild(s, build("B"), 2);
  const r = deleteBuild(s, a.id);
  assert.deepEqual(r.entries.map((e) => e.input), ["B"]);
  assert.equal(r.error, null);
  assert.deepEqual(deleteBuild(s, "unknown").entries.map((e) => e.input), ["B"]);
  assert.equal(clearRecentBuilds(s), null);
  assert.deepEqual(loadRecentBuilds(s).entries, []);
  assert.match(
    clearRecentBuilds({
      removeItem() {
        throw new Error("nope");
      },
    }),
    /nope/,
  );
});

test("formatShortTime is deterministic", () => {
  const now = Date.UTC(2026, 9, 8, 12, 0, 0);
  assert.equal(formatShortTime(now - 5000, now), "just now");
  assert.equal(formatShortTime(now + 5000, now), "just now");
  assert.equal(formatShortTime(now - 5 * 60000, now), "5m ago");
  assert.equal(formatShortTime(now - 3 * 3600000, now), "3h ago");
  assert.equal(formatShortTime(now - 2 * 86400000, now), "2d ago");
  assert.equal(formatShortTime(Date.UTC(2026, 8, 1, 23, 59), now), "2026-09-01");
  assert.equal(formatShortTime(NaN, now), "");
});

test("previewInput gives at most two truncated lines", () => {
  assert.deepEqual(previewInput("H|a\r\nP|1\nL|1"), ["H|a", "P|1 L|1"]);
  assert.deepEqual(previewInput("single"), ["single"]);
  assert.deepEqual(previewInput(""), []);
  const [one] = previewInput("x".repeat(200), 10);
  assert.equal(one.length, 10);
  assert.ok(one.endsWith("…"));
});

test("out-of-range lastUsed is sanitized to 0 and formatShortTime never throws", () => {
  const s = memoryStorage({
    [RECENT_BUILDS_KEY]: JSON.stringify([
      { id: "a", input: "A", lastUsed: -1e20 },
      { id: "b", input: "B", lastUsed: 1e20 },
      { id: "c", input: "C", lastUsed: 8.64e15 },
    ]),
  });
  const { entries } = loadRecentBuilds(s);
  assert.equal(entries.find((e) => e.id === "a").lastUsed, 0);
  assert.equal(entries.find((e) => e.id === "b").lastUsed, 0);
  assert.equal(entries.find((e) => e.id === "c").lastUsed, 8.64e15);
  for (const v of [-1e20, 1e20, NaN, Infinity, "x", undefined, 8.64e15 + 1]) {
    assert.doesNotThrow(() => formatShortTime(v, 1000));
  }
  assert.equal(formatShortTime(-1e20, 1000), "");
  assert.doesNotThrow(() => formatShortTime(0, 1e20));
});

test("entries lacking or duplicating ids get stable ids and delete works first time", () => {
  const s = memoryStorage({
    [RECENT_BUILDS_KEY]: JSON.stringify([
      { input: "A", lastUsed: 3 },
      { id: "dup", input: "B", lastUsed: 2 },
      { id: "dup", input: "C", lastUsed: 1 },
    ]),
  });
  const first = loadRecentBuilds(s).entries;
  const second = loadRecentBuilds(s).entries;
  assert.deepEqual(first.map((e) => e.id), second.map((e) => e.id));
  assert.equal(new Set(first.map((e) => e.id)).size, 3);
  const victim = first[0];
  const after = deleteBuild(s, victim.id).entries;
  assert.deepEqual(after.map((e) => e.input), ["B", "C"]);
  assert.deepEqual(loadRecentBuilds(s).entries.map((e) => e.input), ["B", "C"]);
  const dupDeleted = deleteBuild(s, loadRecentBuilds(s).entries[1].id).entries;
  assert.deepEqual(dupDeleted.map((e) => e.input), ["B"]);
});
