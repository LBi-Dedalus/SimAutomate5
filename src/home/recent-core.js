// Recent endpoints kept in localStorage. Pure module: the storage is injected.

export const RECENT_KEY = "simautomate:recent-endpoints";
export const MAX_RECENT = 8;

/**
 * Validates and normalizes an endpoint. Server endpoints have no remote host.
 * Returns null when the endpoint is not usable.
 */
export function normalizeEndpoint(input) {
  if (!input || (input.mode !== "client" && input.mode !== "server")) {
    return null;
  }
  const port = Number(input.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null;

  if (input.mode === "server") {
    return { mode: "server", host: "", port };
  }
  const host = String(input.host ?? "").trim().toLowerCase();
  if (host === "" || /\s/.test(host)) return null;
  return { mode: "client", host, port };
}

export function endpointKey(endpoint) {
  return `${endpoint.mode}|${endpoint.host}|${endpoint.port}`;
}

function sanitize(list) {
  const seen = new Set();
  const entries = [];
  for (const item of list) {
    const endpoint = normalizeEndpoint(item);
    if (!endpoint) continue;
    const key = endpointKey(endpoint);
    if (seen.has(key)) continue;
    seen.add(key);
    const lastUsed = Number.isFinite(item.lastUsed) ? item.lastUsed : 0;
    entries.push({ ...endpoint, lastUsed });
  }
  entries.sort((a, b) => b.lastUsed - a.lastUsed);
  return entries.slice(0, MAX_RECENT);
}

/** Never throws: corrupt or unavailable storage yields an empty list plus an error. */
export function loadRecent(storage) {
  try {
    const raw = storage.getItem(RECENT_KEY);
    if (raw === null || raw === undefined) return { entries: [], error: null };
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return { entries: [], error: "Recent endpoints storage is not a list" };
    }
    return { entries: sanitize(parsed), error: null };
  } catch (err) {
    return { entries: [], error: `Recent endpoints unreadable: ${String(err)}` };
  }
}

/** Records an endpoint as most recent. Returns { entries, error }. */
export function recordRecent(storage, endpoint, now = Date.now()) {
  const normalized = normalizeEndpoint(endpoint);
  if (!normalized) {
    return { entries: loadRecent(storage).entries, error: "Invalid endpoint" };
  }
  const { entries } = loadRecent(storage);
  const key = endpointKey(normalized);
  const next = sanitize([
    { ...normalized, lastUsed: now },
    ...entries.filter((entry) => endpointKey(entry) !== key),
  ]);
  try {
    storage.setItem(RECENT_KEY, JSON.stringify(next));
    return { entries: next, error: null };
  } catch (err) {
    return { entries: next, error: `Cannot save recent endpoints: ${String(err)}` };
  }
}

export function clearRecent(storage) {
  try {
    storage.removeItem(RECENT_KEY);
    return null;
  } catch (err) {
    return `Cannot clear recent endpoints: ${String(err)}`;
  }
}

export function describeEndpoint(endpoint) {
  return endpoint.mode === "server"
    ? `Server :${endpoint.port}`
    : `${endpoint.host}:${endpoint.port}`;
}
