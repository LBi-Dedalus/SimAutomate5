import {
  normalizeEndpoint,
  loadRecent,
  recordRecent,
  clearRecent,
  describeEndpoint,
} from "./recent-core.js";
import { el } from "./render.js";
import { logWarn } from "./log.js";
import { sessions, store } from "./sessions.js";
import { showView } from "./nav.js";

let mode = "client";
/** Guards against a double submit while the backend answers; it never blocks other sessions. */
let submitting = false;

window.addEventListener("DOMContentLoaded", init);

function init() {
  initModeButtons();
  initConnectForm();
  document.getElementById("recent-clear").addEventListener("click", () => {
    const error = clearRecent(localStorage);
    showRecentNote(error);
    renderRecent();
  });
  // A recent endpoint is remembered once its session is really up (connected / listening).
  store.subscribe((event) => {
    if (event.type === "status") recordWhenUp(event.session);
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

/** Home only configures the NEXT connection: the mode can always be changed. */
function setMode(next) {
  mode = next;
  const client = next === "client";

  document.getElementById("client-mode").classList.toggle("on", client);
  document.getElementById("server-mode").classList.toggle("on", !client);
  document.getElementById("client-host-field").classList.toggle("hidden", !client);
  document.getElementById("client-port-field").classList.toggle("hidden", !client);
  document.getElementById("server-port-field").classList.toggle("hidden", client);

  syncModeFields();
  document.getElementById("connect-btn").textContent = client ? "Connect" : "Start server";
}

/**
 * Only the fields of the active mode take part in validation: inactive ones are disabled
 * (a stale invalid value in a hidden field can neither block nor satisfy the active form).
 */
function syncModeFields() {
  const client = mode === "client";
  const form = document.getElementById("connect-form");
  for (const [field, active] of [
    [form.host, client],
    [form.port, client],
    [form["server-port"], !client],
  ]) {
    field.disabled = !active;
    field.required = active;
  }
}

// ── Starting sessions (manual form and recent shortcuts share one path) ──

function initConnectForm() {
  const form = document.getElementById("connect-form");
  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    submitForm();
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

function submitForm() {
  const form = document.getElementById("connect-form");
  showConnectError(null);
  if (submitting) return;
  if (!form.reportValidity()) return;

  if (mode === "client") {
    startSession({ mode, host: form.host.value.trim(), port: Number(form.port.value) });
  } else {
    startSession({ mode, host: "", port: Number(form["server-port"].value) });
  }
}

/** Builds the backend request and opens one more session (never touches the others). */
function startSession({ mode: sessionMode, host, port }) {
  const endpoint = normalizeEndpoint({ mode: sessionMode, host, port });
  if (!endpoint) {
    showConnectError("Enter a valid host and a port between 1 and 65535.");
    return;
  }
  const req =
    sessionMode === "client"
      ? { type: "ClientConnectRequest", ip: host, port }
      : { type: "ServerStartRequest", port };

  // The session is created only once the view is really shown, so a template-editor
  // guard that defers the navigation also defers (or cancels) the new session.
  showView("session", {
    onActivate: () => {
      submitting = true;
      const button = document.getElementById("connect-btn");
      button.disabled = true;
      const { done } = sessions.start({
        mode: sessionMode,
        label: describeEndpoint(endpoint),
        endpoint,
        req,
      });
      void done.finally(() => {
        submitting = false;
        button.disabled = false;
        syncModeFields();
      });
    },
  });
}

/** Records a recent endpoint only after the connection/listening is confirmed. */
function recordWhenUp(session) {
  if (!session || session.recorded || !session.endpoint) return;
  const up =
    (session.mode === "client" && session.status === "connected") ||
    (session.mode === "server" && session.status === "listening");
  if (!up) return;
  store.markRecorded(session.id);
  const { error } = recordRecent(localStorage, session.endpoint);
  if (error) {
    logWarn(error, "connection.js:recordRecent");
    showRecentNote(error);
  }
  renderRecent();
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
    item.title = `${client ? "Connect to" : "Listen on"} ${endpointText(entry)} in a new session`;
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

/** A shortcut always opens a NEW session, whatever the other sessions are doing. */
function useRecent(entry) {
  showConnectError(null);
  startSession({ mode: entry.mode, host: entry.host, port: entry.port });
}
