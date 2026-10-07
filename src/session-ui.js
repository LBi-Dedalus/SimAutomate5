// Sidebar "Sessions" list, session header (title, status, stats, actions) and the
// close confirmation. Everything is rendered from the session store.
import { el } from "./render.js";
import { sessions, store } from "./sessions.js";
import { showView } from "./nav.js";
import { isBusyStatus, isTerminalStatus } from "./sessions-core.js";

const STATUS_TEXT = {
  disconnected: "Disconnected",
  connecting: "Connecting",
  listening: "Listening",
  connected: "Connected",
  error: "Error",
};

const $ = (id) => document.getElementById(id);

window.addEventListener("DOMContentLoaded", init);

function init() {
  $("session-new").addEventListener("click", () => {
    // Home is where new connections are configured; the navigation guard still applies.
    showView("home");
  });
  $("session-disconnect").addEventListener("click", () => {
    const id = store.activeId;
    if (id) void sessions.disconnect(id);
  });
  $("session-reconnect").addEventListener("click", () => {
    const id = store.activeId;
    if (id) void sessions.reconnect(id);
  });
  $("session-close").addEventListener("click", () => {
    const id = store.activeId;
    if (id) void requestClose(id);
  });
  store.subscribe((event) => {
    if (event.type === "draft" || event.type === "record-selected") {
      // Only the close impact depends on these; nothing visible changes.
      return;
    }
    if (event.type === "message") {
      // Streaming must not rebuild (and drop the focus of) the sidebar on every line.
      renderHeader();
      if (event.session.unread > 0) renderSidebar();
      return;
    }
    render();
  });
  render();
}

/** Selecting a session is a navigation: the active session only changes once the view is shown. */
export function openSession(id) {
  showView("session", {
    onActivate: () => {
      if (!store.select(id)) showNotice("That session has been closed.");
    },
  });
}

function showNotice(message) {
  const node = $("sessions-note");
  node.textContent = message ?? "";
  node.classList.toggle("hidden", !message);
}

/** Asks before dropping a running connection, a history or an unsent draft. */
export async function requestClose(id) {
  const impact = sessions.closeImpact(id);
  if (!impact) return;
  if (impact.needsConfirm && !(await confirmClose(impact))) return;
  const result = await sessions.close(id);
  if (!result.ok) showNotice(`Could not close the session: ${result.error}`);
}

function confirmClose(impact) {
  const dialog = $("close-confirm");
  if (dialog.open) return Promise.resolve(false);

  const consequences = [];
  if (impact.running) consequences.push("The connection will be stopped.");
  if (impact.messages > 0) {
    consequences.push(
      `Its ${impact.messages} message${impact.messages === 1 ? "" : "s"} of history will be lost.`,
    );
  }
  if (impact.draft) consequences.push("The unsent text in its composer will be lost.");
  $("close-confirm-title").textContent = `Close “${impact.label}”?`;
  const list = $("close-confirm-list");
  list.replaceChildren(...consequences.map((text) => el("li", "", text)));

  return new Promise((resolve) => {
    dialog.addEventListener(
      "close",
      () => resolve(dialog.returnValue === "confirm"),
      { once: true },
    );
    dialog.returnValue = "cancel";
    dialog.showModal();
  });
}

function render() {
  renderSidebar();
  renderHeader();
}

function endpointLabel(session) {
  return session.label;
}

function renderSidebar() {
  const list = $("side-sessions");
  const all = store.list();
  list.replaceChildren();
  let activeItem = null;
  $("side-sessions-empty").classList.toggle("hidden", all.length > 0);

  for (const session of all) {
    const active = session.id === store.activeId;
    const item = el("div", "ses-item");
    item.classList.toggle("on", active);

    const open = el("button", "ses");
    open.type = "button";
    open.dataset.sessionId = session.id;
    if (active) open.setAttribute("aria-current", "true");
    const dot = el("i");
    dot.dataset.st = session.status;
    open.appendChild(dot);
    const tx = el("span", "tx");
    tx.appendChild(el("span", "", endpointLabel(session)));
    tx.appendChild(
      el("small", "", `${session.mode === "client" ? "Client" : "Server"} · ${STATUS_TEXT[session.status] ?? session.status}`),
    );
    open.appendChild(tx);
    if (session.unread > 0 && !active) {
      open.appendChild(el("em", "ub", String(Math.min(session.unread, 99))));
    }
    open.addEventListener("click", () => openSession(session.id));
    item.appendChild(open);

    const close = el("button", "ses-x", "×");
    close.type = "button";
    close.title = `Close ${session.label}`;
    close.setAttribute("aria-label", `Close session ${session.label}`);
    close.disabled = session.op === "closing";
    close.addEventListener("click", () => void requestClose(session.id));
    item.appendChild(close);

    list.appendChild(item);
    if (active) activeItem = item;
  }
  // Keep the selected row reachable in a long, scrolling list.
  activeItem?.scrollIntoView?.({ block: "nearest" });

  const active = store.active();
  $("side-endpoint").textContent = active ? active.label : "";
}

function renderHeader() {
  const session = store.active();
  const busy = !!session && isBusyStatus(session.status);
  const terminal = !!session && isTerminalStatus(session.status);
  const working = !!session?.op;

  $("session-title").textContent = session ? session.label : "No session";
  const badge = $("session-mode");
  badge.classList.toggle("hidden", !session);
  badge.textContent = session?.mode === "server" ? "Server" : "Client";

  const stats = $("session-stats");
  const sent = session?.sent ?? 0;
  const received = session?.received ?? 0;
  stats.textContent = `↑ ${sent} sent · ↓ ${received} received`;
  stats.classList.toggle("hidden", sent + received === 0);

  const disconnect = $("session-disconnect");
  disconnect.classList.toggle("hidden", !busy);
  disconnect.disabled = working;
  disconnect.textContent = session?.mode === "server" ? "Stop server" : "Disconnect";

  const reconnect = $("session-reconnect");
  reconnect.classList.toggle("hidden", !terminal);
  reconnect.disabled = working;
  reconnect.textContent = session?.mode === "server" ? "Restart server" : "Reconnect";

  const close = $("session-close");
  close.classList.toggle("hidden", !session);
  close.disabled = session?.op === "closing";

  $("clear-chat").disabled = !session;
}
