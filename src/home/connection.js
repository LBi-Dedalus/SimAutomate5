import {
  normalizeEndpoint,
  loadRecent,
  recordRecent,
  clearRecent,
  describeEndpoint,
  endpointText,
  isServerMode,
  isSshMode,
  modeLabel,
  MODES,
} from "./recent-core.js";
import { cloneTemplate } from "../include.js";
import { logWarn } from "../log.js";
import { sessions, store } from "../session/sessions.js";
import { showView } from "../nav.js";

// Preloaded by the <link rel="x-template"> tags of home.html.
const CARD_TEMPLATE = "home/recent-card.html";
const SIDE_ITEM_TEMPLATE = "home/recent-side-item.html";

let mode = "client";
/** Guards against a double submit while the backend answers; it never blocks other sessions. */
let submitting = false;

function init() {
  // A recent endpoint is remembered once its session is really up (connected / listening).
  store.subscribe((event) => {
    if (event.type === "status") recordWhenUp(event.session);
  });
  setMode("client");
  renderRecent();
}

// ── Mode ────────────────────────────────────────────────────

const SSH_KEY_HINT =
  "The SSH key comes from ~/.ssh/config or the default key files (~/.ssh/id_ed25519, id_ecdsa, id_rsa) and must have no passphrase.";

const MODE_UI = {
  client: { button: "Connect", hint: null },
  "ssh-client": { button: "Connect via SSH", hint: SSH_KEY_HINT },
  server: {
    button: "Start server",
    hint: "Server mode listens on all interfaces (0.0.0.0) and accepts one client.",
  },
  "ssh-server": {
    button: "Start server via SSH",
    hint: `SSH server mode listens on the VM's 127.0.0.1 and accepts one client. ${SSH_KEY_HINT}`,
  },
};

/** Home only configures the NEXT connection: the mode can always be changed. */
function setMode(next) {
  mode = next;
  const server = isServerMode(next);
  const ui = MODE_UI[next];

  for (const id of MODES) {
    document.getElementById(`${id}-mode`).classList.toggle("on", id === next);
  }
  document.getElementById("client-host-field").classList.toggle("hidden", server);
  document.getElementById("client-port-field").classList.toggle("hidden", server);
  document.getElementById("server-port-field").classList.toggle("hidden", !server);
  document.getElementById("ssh-fields").classList.toggle("hidden", !isSshMode(next));
  const hint = document.getElementById("connect-hint");
  hint.textContent = ui.hint ?? "";
  hint.classList.toggle("hidden", !ui.hint);

  syncModeFields();
  document.getElementById("connect-btn").textContent = ui.button;
}

/**
 * Only the fields of the active mode take part in validation: inactive ones are disabled
 * (a stale invalid value in a hidden field can neither block nor satisfy the active form).
 * The SSH user is never required: the backend resolves an empty one.
 */
function syncModeFields() {
  const server = isServerMode(mode);
  const ssh = isSshMode(mode);
  const form = document.getElementById("connect-form");
  for (const [field, active, required] of [
    [form.host, !server, !server],
    [form.port, !server, !server],
    [form["server-port"], server, server],
    [form["ssh-host"], ssh, ssh],
    [form["ssh-port"], ssh, ssh],
    [form["ssh-user"], ssh, false],
  ]) {
    field.disabled = !active;
    field.required = required;
  }
}

// ── Starting sessions (manual form and recent shortcuts share one path) ──

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

  const server = isServerMode(mode);
  startSession({
    mode,
    host: server ? "" : form.host.value.trim(),
    port: Number(server ? form["server-port"].value : form.port.value),
    ...(isSshMode(mode) && {
      sshHost: form["ssh-host"].value.trim(),
      sshPort: Number(form["ssh-port"].value),
      sshUser: form["ssh-user"].value.trim(),
    }),
  });
}

/** The backend request (serde tag "type") of a normalized endpoint. */
function buildRequest(endpoint) {
  const { mode: m, host, port, sshHost, sshPort, sshUser } = endpoint;
  const ssh = { ssh_host: sshHost, ssh_port: sshPort, ssh_user: sshUser };
  switch (m) {
    case "client":
      return { type: "ClientConnectRequest", ip: host, port };
    case "ssh-client":
      return { type: "SshClientConnectRequest", ip: host, port, ...ssh };
    case "server":
      return { type: "ServerStartRequest", port };
    default:
      return { type: "SshServerStartRequest", port, ...ssh };
  }
}

/** Builds the backend request and opens one more session (never touches the others). */
function startSession(input) {
  const endpoint = normalizeEndpoint(input);
  if (!endpoint) {
    showConnectError(
      isSshMode(input.mode)
        ? "Enter a valid host, a port and an SSH host and SSH port between 1 and 65535."
        : "Enter a valid host and a port between 1 and 65535.",
    );
    return;
  }
  const sessionMode = endpoint.mode;
  const req = buildRequest(endpoint);

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
    (!isServerMode(session.mode) && session.status === "connected") ||
    (isServerMode(session.mode) && session.status === "listening");
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
  document.getElementById("recent-clear").classList.toggle("hidden", entries.length === 0);
  document.getElementById("side-recent-empty").classList.toggle("hidden", entries.length > 0);

  for (const entry of entries) {
    const client = !isServerMode(entry.mode);
    const card = cloneTemplate(CARD_TEMPLATE, {
      kind: modeLabel(entry.mode),
      verb: client ? "Connect " : "Listen ",
      endpoint: endpointText(entry),
    });
    card.querySelector(".badge").classList.toggle("mute", !client);
    Object.assign(card.dataset, entryData(entry));
    list.appendChild(card);
  }

  for (const entry of entries.slice(0, 5)) {
    const client = !isServerMode(entry.mode);
    const item = cloneTemplate(SIDE_ITEM_TEMPLATE, {
      endpoint: endpointText(entry),
      detail: [modeLabel(entry.mode), relativeTime(entry.lastUsed)].filter(Boolean).join(" · "),
    });
    item.title = `${client ? "Connect to" : "Listen on"} ${endpointText(entry)} in a new session`;
    Object.assign(item.dataset, entryData(entry));
    side.appendChild(item);
  }
}

/** The recent endpoint carried by a card / side item (read back by actions.home.useRecent). */
const entryData = ({ mode: m, host, port, sshHost, sshPort, sshUser }) => ({
  mode: m,
  host,
  port,
  ...(isSshMode(m) && { sshHost, sshPort, sshUser }),
});

/** A shortcut always opens a NEW session, whatever the other sessions are doing. */
function useRecent(entry) {
  showConnectError(null);
  startSession({
    mode: entry.mode,
    host: entry.host,
    port: Number(entry.port),
    sshHost: entry.sshHost,
    sshPort: Number(entry.sshPort),
    sshUser: entry.sshUser,
  });
}

window.actions ??= {};
window.actions.home = {
  setClient: () => setMode("client"),
  setSshClient: () => setMode("ssh-client"),
  setServer: () => setMode("server"),
  setSshServer: () => setMode("ssh-server"),
  submit: (ev) => {
    ev.preventDefault();
    submitForm();
  },
  clearRecent: () => {
    const error = clearRecent(localStorage);
    showRecentNote(error);
    renderRecent();
  },
  useRecent: (ev, el) => useRecent(el.dataset),
};

init();
