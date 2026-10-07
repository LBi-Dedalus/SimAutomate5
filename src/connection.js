import {
  normalizeEndpoint,
  loadRecent,
  recordRecent,
  clearRecent,
  describeEndpoint,
} from "./recent-core.js";
import { el } from "./render.js";
import { logError, logWarn } from "./log.js";

const { invoke } = window.__TAURI__.core;

let mode = "client";
/** Endpoint of the attempt in progress; recorded only once it is really up. */
let pending = null;
let activeLabel = "";

window.addEventListener("DOMContentLoaded", init);

function init() {
  initModeButtons();
  initConnectForm();
  initStatusHandling();
  document
    .getElementById("session-disconnect")
    .addEventListener("click", submitConnection);
  document.getElementById("recent-clear").addEventListener("click", () => {
    const error = clearRecent(localStorage);
    showRecentNote(error);
    renderRecent();
  });
  setMode("client");
  renderRecent();
}

// ── Mode ────────────────────────────────────────────────────

function initModeButtons() {
  document
    .getElementById("client-mode")
    .addEventListener("click", () => setMode("client"));
  document
    .getElementById("server-mode")
    .addEventListener("click", () => setMode("server"));
}

/** Returns false (and changes nothing) while connecting, listening or connected. */
function setMode(next) {
  if (window.connection_status.isBusy()) return false;
  mode = next;
  const client = next === "client";

  document.getElementById("client-mode").classList.toggle("on", client);
  document.getElementById("server-mode").classList.toggle("on", !client);
  document.getElementById("client-host-field").classList.toggle("hidden", !client);
  document.getElementById("client-port-field").classList.toggle("hidden", !client);
  document.getElementById("server-port-field").classList.toggle("hidden", client);

  // Hidden fields must not block validation.
  const form = document.getElementById("connect-form");
  form.host.required = client;
  form.port.required = client;
  form["server-port"].required = !client;
  return true;
}

// ── Connect / disconnect (single handler for manual and recent use) ──

function initConnectForm() {
  const form = document.getElementById("connect-form");
  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    void submitConnection();
  });
}

function setField(name, value) {
  const field = document.getElementById("connect-form")[name];
  field.value = value;
  // Lets the existing config persistence store the value.
  field.dispatchEvent(new Event("change", { bubbles: true }));
}

function showConnectError(message) {
  const node = document.getElementById("connect-error");
  node.textContent = message ?? "";
  node.classList.toggle("hidden", !message);
}

async function submitConnection() {
  const form = document.getElementById("connect-form");
  showConnectError(null);

  if (window.connection_status.isBusy()) {
    try {
      console.log(mode === "client" ? "Disconnect requested" : "Server stop requested");
      await invoke("disconnect_socket");
    } catch (err) {
      console.error("Failed to disconnect", err);
      logError(`Failed to disconnect: ${String(err)}`, "connection.js:disconnect");
      showConnectError(`Failed to disconnect: ${String(err)}`);
    }
    return;
  }

  let req;
  let endpoint;
  if (mode === "client") {
    if (!form.reportValidity()) return;
    const host = form.host.value;
    const port = Number(form.port.value);
    req = { type: "ClientConnectRequest", ip: host, port };
    endpoint = normalizeEndpoint({ mode, host, port });
    console.log(`Connect requested (host=${host}, port=${port})`);
  } else {
    if (!form.reportValidity()) return;
    const port = Number(form["server-port"].value);
    req = { type: "ServerStartRequest", port };
    endpoint = normalizeEndpoint({ mode, host: "", port });
    console.log(`Server start requested (port=${port})`);
  }

  pending = endpoint;
  activeLabel = endpoint
    ? describeEndpoint(endpoint)
    : mode === "client"
      ? `${form.host.value}:${form.port.value}`
      : `Server :${form["server-port"].value}`;
  try {
    await invoke("connect_socket", { req });
  } catch (err) {
    pending = null;
    console.error("Failed to connect", err);
    logError(`Failed to connect: ${String(err)}`, "connection.js:connect");
    showConnectError(`Failed to connect: ${String(err)}`);
  }
}

// ── Status handling ─────────────────────────────────────────

function initStatusHandling() {
  window.connection_status.subscribe(onStatus);
  onStatus(window.connection_status.get());
}

function onStatus(status) {
  settlePending(status);
  applyBusyUi(status);
  renderRecent();
}

/** Records a recent endpoint only after the connection/listening is confirmed. */
function settlePending(status) {
  if (!pending) return;
  const confirmed =
    (pending.mode === "client" && status === "connected") ||
    (pending.mode === "server" && status === "listening");
  if (confirmed) {
    const endpoint = pending;
    pending = null;
    const { error } = recordRecent(localStorage, endpoint);
    if (error) {
      logWarn(error, "connection.js:recordRecent");
      showRecentNote(error);
    }
    renderRecent();
  } else if (status === "error" || status === "disconnected") {
    pending = null;
  }
}

function applyBusyUi(status) {
  const busy = window.connection_status.isBusy();
  const form = document.getElementById("connect-form");

  document.getElementById("client-mode").disabled = busy;
  document.getElementById("server-mode").disabled = busy;
  for (const field of form.querySelectorAll("input")) field.disabled = busy;

  const button = document.getElementById("connect-btn");
  button.textContent = busy
    ? mode === "client"
      ? "Disconnect"
      : "Stop server"
    : mode === "client"
      ? "Connect"
      : "Start server";
  button.dataset.variant = busy ? "secondary" : "primary";

  for (const card of document.querySelectorAll("#recent-list button, #side-recent button")) {
    card.disabled = busy;
  }
  document.getElementById("no-connection").toggleAttribute("data-busy", busy);

  // Session header + sidebar
  const title = document.getElementById("session-title");
  const badge = document.getElementById("session-mode");
  const disconnect = document.getElementById("session-disconnect");
  title.textContent = busy ? activeLabel || "Session" : "No connection";
  badge.classList.toggle("hidden", !busy);
  badge.textContent = mode === "client" ? "Client" : "Server";
  disconnect.classList.toggle("hidden", !busy);
  disconnect.textContent = mode === "client" ? "Disconnect" : "Stop server";
  document.getElementById("side-endpoint").textContent = busy ? activeLabel : "";
}

// ── Recent endpoints ────────────────────────────────────────

function showRecentNote(message) {
  const node = document.getElementById("recent-note");
  node.textContent = message ?? "";
  node.classList.toggle("hidden", !message);
}

function relativeTime(timestamp) {
  const diff = Date.now() - new Date(timestamp).getTime();
  if (!Number.isFinite(diff)) return "";
  const minutes = Math.round(diff / 60000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  return new Date(timestamp).toLocaleDateString();
}

function endpointText(entry) {
  return entry.mode === "client" ? `${entry.host}:${entry.port}` : `:${entry.port}`;
}

function renderRecent() {
  const list = document.getElementById("recent-list");
  const side = document.getElementById("side-recent");
  const { entries, error } = loadRecent(localStorage);
  if (error) {
    logWarn(error, "connection.js:loadRecent");
    showRecentNote(error);
  }

  const busy = window.connection_status.isBusy();
  list.replaceChildren();
  side.replaceChildren();
  document.getElementById("recent-empty").classList.toggle("hidden", entries.length > 0);
  document.getElementById("recent-clear").classList.toggle("hidden", entries.length === 0);
  document.getElementById("side-recent-empty").classList.toggle("hidden", entries.length > 0);

  for (const entry of entries) {
    const client = entry.mode === "client";
    const when = relativeTime(entry.lastUsed);

    const card = el("button", "rc");
    card.type = "button";
    card.disabled = busy;
    const top = el("div", "top");
    top.appendChild(el("span", client ? "tag" : "tag c", client ? "Client" : "Server"));
    top.appendChild(el("span", "go", client ? "Connect ↗" : "Listen ↗"));
    card.appendChild(top);
    card.appendChild(el("div", "ep", endpointText(entry)));
    card.appendChild(el("div", "ds", client ? "Connect as client" : "Listen on all interfaces"));
    card.appendChild(el("div", "bt", when ? `Last used ${when}` : ""));
    card.addEventListener("click", () => useRecent(entry));
    list.appendChild(card);
  }

  for (const entry of entries.slice(0, 5)) {
    const client = entry.mode === "client";
    const item = el("button", "ses");
    item.type = "button";
    item.disabled = busy;
    item.title = busy ? "Disconnect first" : `Connect to ${endpointText(entry)}`;
    item.classList.toggle("on", busy && describeEndpoint(entry) === activeLabel);
    item.appendChild(el("i"));
    const tx = el("span", "tx");
    tx.appendChild(el("span", "", endpointText(entry)));
    tx.appendChild(el("small", "", [client ? "Client" : "Server", relativeTime(entry.lastUsed)].filter(Boolean).join(" · ")));
    item.appendChild(tx);
    item.appendChild(el("span", "x", "↗"));
    item.addEventListener("click", () => useRecent(entry));
    side.appendChild(item);
  }
}

function useRecent(entry) {
  // Cannot switch endpoint while connecting, listening or connected.
  if (!setMode(entry.mode)) return;
  if (entry.mode === "client") {
    setField("host", entry.host);
    setField("port", String(entry.port));
  } else {
    setField("server-port", String(entry.port));
  }
  void submitConnection();
}
