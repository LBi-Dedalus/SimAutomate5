// Recent autobuilds kept in localStorage. Pure module: the storage is injected.

export const RECENT_BUILDS_KEY = "simautomate:recent-autobuilds";
export const MAX_RECENT_BUILDS = 20;

/** Mirrors the backend detection (message_builder.rs): `H|` is ASTM, `MSH|` is HL7. */
export function detectKind(input) {
  const text = String(input ?? "").trimStart();
  if (text.startsWith("H|")) return "ASTM";
  if (text.startsWith("MSH|")) return "HL7";
  return "Raw";
}

export function buildKey(input, noEtb) {
  return `${noEtb ? 1 : 0}|${input}`;
}

let counter = 0;
function newId(now) {
  counter += 1;
  return `${now.toString(36)}-${counter.toString(36)}`;
}

const MAX_DATE_MS = 8.64e15;
const validTime = (v) => typeof v === "number" && Number.isFinite(v) && Math.abs(v) <= MAX_DATE_MS;

/** Deterministic id for entries lacking a usable one: stable across loads (FNV-1a of the key). */
function repairedId(input, noEtb, seenIds) {
  const key = buildKey(input, noEtb);
  let h = 0x811c9dc5;
  for (let i = 0; i < key.length; i += 1) {
    h ^= key.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  const base = `r-${h.toString(36)}`;
  let id = base;
  for (let n = 2; seenIds.has(id); n += 1) id = `${base}-${n}`;
  return id;
}

function sanitize(list) {
  const seenKeys = new Set();
  const seenIds = new Set();
  const entries = [];
  const sorted = list
    .filter((item) => item && typeof item === "object" && typeof item.input === "string")
    .map((item) => ({ item, lastUsed: validTime(item.lastUsed) ? item.lastUsed : 0 }))
    .sort((a, b) => b.lastUsed - a.lastUsed);
  for (const { item, lastUsed } of sorted) {
    if (item.input.trim() === "") continue;
    const noEtb = item.noEtb === true;
    const key = buildKey(item.input, noEtb);
    if (seenKeys.has(key)) continue;
    seenKeys.add(key);
    let id = typeof item.id === "string" && item.id !== "" ? item.id : "";
    if (id === "" || seenIds.has(id)) id = repairedId(item.input, noEtb, seenIds);
    seenIds.add(id);
    entries.push({
      id,
      input: item.input,
      output: typeof item.output === "string" ? item.output : "",
      noEtb,
      kind: detectKind(item.input),
      lastUsed,
    });
  }
  return entries.slice(0, MAX_RECENT_BUILDS);
}

/** Never throws: corrupt or unavailable storage yields an empty list plus an error. */
export function loadRecentBuilds(storage) {
  try {
    const raw = storage.getItem(RECENT_BUILDS_KEY);
    if (raw === null || raw === undefined) return { entries: [], error: null };
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return { entries: [], error: "Recent autobuilds storage is not a list" };
    }
    return { entries: sanitize(parsed), error: null };
  } catch (err) {
    return { entries: [], error: `Recent autobuilds unreadable: ${String(err)}` };
  }
}

/**
 * Persists the list. When the storage refuses it (quota), the oldest entries are dropped
 * until it fits. Returns { entries (what is really stored), error }.
 */
function save(storage, entries) {
  let kept = entries;
  let lastError = null;
  while (true) {
    try {
      storage.setItem(RECENT_BUILDS_KEY, JSON.stringify(kept));
      return { entries: kept, error: null };
    } catch (err) {
      lastError = err;
      if (kept.length <= 1) break;
      kept = kept.slice(0, -1);
    }
  }
  return { entries, error: `Cannot save recent autobuilds: ${String(lastError)}` };
}

/**
 * Records a successful build as the most recent one. The same input + No ETB moves to the
 * top (same id) with a refreshed output. Empty input is ignored.
 * Returns { entries, error, entry } (entry is null when nothing was recorded).
 */
export function recordBuild(storage, { input, output, noEtb }, now = Date.now()) {
  const { entries } = loadRecentBuilds(storage);
  if (typeof input !== "string" || input.trim() === "") {
    return { entries, error: null, entry: null };
  }
  const flag = noEtb === true;
  const key = buildKey(input, flag);
  const existing = entries.find((e) => buildKey(e.input, e.noEtb) === key);
  const entry = {
    id: existing ? existing.id : newId(now),
    input,
    output: typeof output === "string" ? output : "",
    noEtb: flag,
    kind: detectKind(input),
    lastUsed: now,
  };
  const next = [entry, ...entries.filter((e) => e !== existing)].slice(0, MAX_RECENT_BUILDS);
  const saved = save(storage, next);
  return { entries: saved.entries, error: saved.error, entry };
}

export function deleteBuild(storage, id) {
  const { entries } = loadRecentBuilds(storage);
  const next = entries.filter((e) => e.id !== id);
  const saved = save(storage, next);
  return { entries: saved.entries, error: saved.error };
}

export function clearRecentBuilds(storage) {
  try {
    storage.removeItem(RECENT_BUILDS_KEY);
    return null;
  } catch (err) {
    return `Cannot clear recent autobuilds: ${String(err)}`;
  }
}

/** Short, deterministic relative time ("just now", "5m ago", "3h ago", "2d ago", "YYYY-MM-DD"). */
export function formatShortTime(ts, now = Date.now()) {
  if (!validTime(ts)) return "";
  const diff = Math.max(0, (validTime(now) ? now : Date.now()) - ts);
  const minutes = Math.floor(diff / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  try {
    return new Date(ts).toISOString().slice(0, 10);
  } catch {
    return "";
  }
}

/** Two-line preview: the first non-empty line, then the remainder, each truncated. */
export function previewInput(input, max = 80) {
  const lines = String(input ?? "").split(/\r\n|\r|\n/).filter((l) => l.trim() !== "");
  const cut = (s) => (s.length > max ? `${s.slice(0, max - 1)}…` : s);
  return [cut(lines[0] ?? ""), cut(lines.slice(1).join(" "))].filter((s) => s !== "");
}
