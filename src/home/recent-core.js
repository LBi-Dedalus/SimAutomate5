// Recent endpoints kept in localStorage. Pure module: the storage is injected.

export const RECENT_KEY = "simautomate:recent-endpoints";
export const MAX_RECENT = 8;

export const MODES = ["client", "ssh-client", "server", "ssh-server"];
const MODE_LABELS = {
  client: "Client",
  "ssh-client": "SSH Client",
  server: "Server",
  "ssh-server": "SSH Server",
};

export const isSshMode = (mode) => mode === "ssh-client" || mode === "ssh-server";
export const isServerMode = (mode) => mode === "server" || mode === "ssh-server";
export const modeLabel = (mode) => MODE_LABELS[mode] ?? "Client";

const validPort = (port) => Number.isInteger(port) && port >= 1 && port <= 65535;

/**
 * Validates and normalizes an endpoint. Server endpoints have no remote host. SSH endpoints
 * also carry sshHost, sshPort and sshUser (possibly empty); the other modes carry none of them,
 * so entries persisted before the SSH modes existed stay valid unchanged.
 * Returns null when the endpoint is not usable.
 */
export function normalizeEndpoint(input) {
  if (!input || !MODES.includes(input.mode)) return null;
  const port = Number(input.port);
  if (!validPort(port)) return null;

  let endpoint;
  if (isServerMode(input.mode)) {
    endpoint = { mode: input.mode, host: "", port };
  } else {
    const host = String(input.host ?? "").trim().toLowerCase();
    if (host === "" || /\s/.test(host)) return null;
    endpoint = { mode: input.mode, host, port };
  }
  if (!isSshMode(input.mode)) return endpoint;

  const sshHost = String(input.sshHost ?? "").trim().toLowerCase();
  if (sshHost === "" || /\s/.test(sshHost)) return null;
  const sshPort = Number(input.sshPort);
  if (!validPort(sshPort)) return null;
  const sshUser = String(input.sshUser ?? "").trim();
  return { ...endpoint, sshHost, sshPort, sshUser };
}

export function endpointKey(endpoint) {
  const base = `${endpoint.mode}|${endpoint.host}|${endpoint.port}`;
  return isSshMode(endpoint.mode)
    ? `${base}|${endpoint.sshUser}@${endpoint.sshHost}:${endpoint.sshPort}`
    : base;
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

/** "user@host:22", or "host:22" when the SSH user is left to the backend. */
export function describeSsh(endpoint) {
  const user = endpoint.sshUser ? `${endpoint.sshUser}@` : "";
  return `${user}${endpoint.sshHost}:${endpoint.sshPort}`;
}

/** Compact endpoint text without the mode word: "host:port", ":port", "user@ssh:22 → host:port". */
export function endpointText(endpoint) {
  const target = isServerMode(endpoint.mode)
    ? `:${endpoint.port}`
    : `${endpoint.host}:${endpoint.port}`;
  if (endpoint.mode === "ssh-client") return `${describeSsh(endpoint)} → ${target}`;
  if (endpoint.mode === "ssh-server") return `${target} ← ${describeSsh(endpoint)}`;
  return target;
}

/** Session label: "host:port", "Server :port", "user@ssh:22 → host:port", "Server :port ← user@ssh:22". */
export function describeEndpoint(endpoint) {
  const text = endpointText(endpoint);
  return isServerMode(endpoint.mode) ? `Server ${text}` : text;
}
