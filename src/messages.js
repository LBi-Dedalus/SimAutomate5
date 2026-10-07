import { describeMessage, hl7Ack } from "./inspector-core.js";
import {
  showInspector,
  clearInspector,
  activateInspector,
  forgetInspector,
} from "./inspector.js";
import { renderTokens, el } from "./render.js";
import { sessions, store, subscribeSendEligibility } from "./sessions.js";

/**
 * One conversation container per session (display: contents inside #messages): switching
 * session swaps the container, nothing is rebuilt. Background sessions keep receiving.
 * @type {Map<string, {container: HTMLElement, nodes: Map<number, HTMLElement>}>}
 */
const views = new Map();
let composerPlaceholder = "";

document.addEventListener("DOMContentLoaded", () => {
  initComposer();
  initConversation();
  initSendButton();
});

/** What a send would target right now (captured synchronously by the caller), or null. */
export function activeSendTarget() {
  return sessions.sendTarget();
}

/** Id of the selected session, or null. */
export function activeSessionId() {
  return store.activeId;
}

/**
 * The single send path used by the composer, control buttons and templates.
 * `target` is captured by the caller before any await; it defaults to the selected session.
 * Never falls back to another session. Resolves true when the backend accepted the message.
 */
export async function sendMessage(message, target = sessions.sendTarget()) {
  if (!target) {
    const active = store.active();
    // The failure is shown in the session it concerns, if there is one.
    if (active) store.addLocal(active.id, "systemerror", "Cannot send: the session is not connected.");
    return false;
  }
  const result = await sessions.send(target, message);
  return result.ok;
}

/**
 * Puts text in the composer draft of a session (default: the selected one, captured by the
 * caller before awaiting). Returns false when that session does not exist (any more).
 */
export function setComposerText(text, sessionId = store.activeId) {
  if (!sessionId) return false;
  return store.setDraft(sessionId, text);
}

function composerField() {
  return document.getElementById("message-form").message;
}

function initComposer() {
  const form = document.getElementById("message-form");
  const field = composerField();
  composerPlaceholder = field.placeholder ?? "";

  // Edits belong to the session displayed when they happen.
  const capture = () => {
    const active = store.activeId;
    if (active) store.setDraft(active, field.value);
  };
  field.addEventListener("input", capture);
  field.addEventListener("change", capture);
  // Reset only clears the draft of the selected session (the field has no default text).
  form.addEventListener("reset", () => {
    const active = store.activeId;
    if (active) store.setDraft(active, "");
  });

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    await sendMessage(field.value);
  });
}

function initSendButton() {
  subscribeSendEligibility((target) => {
    const sendButton = document
      .getElementById("message-form")
      .querySelector('button[type="submit"]');
    if (sendButton) sendButton.disabled = !target;
  });
}

function initConversation() {
  document.getElementById("clear-chat").addEventListener("click", () => {
    const active = store.activeId;
    if (active) store.clearMessages(active);
  });
  store.subscribe(onStoreEvent);
  attachActive();
}

function onStoreEvent(event) {
  switch (event.type) {
    case "message":
      onMessage(event);
      break;
    case "cleared":
      views.get(event.id)?.container.replaceChildren();
      views.get(event.id)?.nodes.clear();
      clearInspector(event.id);
      if (event.id === store.activeId) updateEmptyState();
      break;
    case "removed":
      views.get(event.id)?.container.remove();
      views.delete(event.id);
      forgetInspector(event.id);
      break;
    case "select":
      attachActive();
      break;
    case "status":
      if (event.id === store.activeId) updateEmptyState();
      break;
    case "draft":
      if (event.id === store.activeId) showDraft(event.session);
      break;
    case "record-selected":
      onRecordSelected(event);
      break;
    default:
  }
}

function viewFor(session) {
  let view = views.get(session.id);
  if (!view) {
    const container = el("div", "msgs");
    container.dataset.session = session.id;
    view = { container, nodes: new Map() };
    views.set(session.id, view);
    // Messages received before the view existed (should not happen) are not lost.
    for (const record of session.records) addNode(session, view, record);
  }
  return view;
}

/** Displays the selected session: its conversation, draft and inspector. */
function attachActive() {
  const session = store.active();
  const messagesEl = document.getElementById("messages");
  const field = composerField();

  for (const { container } of views.values()) container.remove();
  if (session) {
    messagesEl.appendChild(viewFor(session).container);
  }
  field.disabled = !session;
  field.placeholder = session ? composerPlaceholder : "Open a session to write a message…";
  showDraft(session);
  activateInspector(session ? session.id : null);
  const selected = session?.records.find((record) => record.id === session.selectedRecordId);
  if (selected) showInspector(selected);
  updateEmptyState();
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

function showDraft(session) {
  const field = composerField();
  const text = session ? session.draft : "";
  if (field.value !== text) field.value = text;
}

function updateEmptyState() {
  const session = store.active();
  const empty = document.getElementById("no-connection");
  const busy = !!session && ["connecting", "listening", "connected"].includes(session.status);
  empty.classList.toggle("hidden", !!session && session.records.length > 0);
  empty.toggleAttribute("data-busy", busy);
}

function onRecordSelected({ session, record }) {
  const view = views.get(session.id);
  if (!view) return;
  for (const [id, node] of view.nodes) node.classList.toggle("sel", record !== null && id === record.id);
  if (record) showInspector(record);
  else clearInspector(session.id);
}

function onMessage({ session, record, evicted }) {
  const view = viewFor(session);
  if (!view.nodes.has(record.id)) addNode(session, view, record);
  for (const old of evicted) {
    view.nodes.get(old.id)?.remove();
    view.nodes.delete(old.id);
    if (session.selectedRecordId === null) clearInspector(session.id);
  }
  if (session.id === store.activeId) {
    updateEmptyState();
    const messagesEl = document.getElementById("messages");
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }
}

function addNode(session, view, record) {
  const info = describeMessage(record);
  const node =
    info.kind === "system" ? buildSystemNode(record) : buildBubbleNode(record, info);

  node.tabIndex = 0;
  const choose = () => store.selectRecord(session.id, record.id);
  node.addEventListener("click", choose);
  node.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" || ev.key === " ") {
      ev.preventDefault();
      choose();
    }
  });
  view.container.appendChild(node);
  view.nodes.set(record.id, node);
}

function buildSystemNode(record) {
  const node = el("div", `sys ${record.msg_type}`);
  node.appendChild(el("span", "txt", record.content));
  node.appendChild(el("span", "t", formatTime(record.timestamp)));
  return node;
}

function buildBubbleNode(record, info) {
  const outgoing = record.msg_type === "sent";
  const node = el("div", `msg ${outgoing ? "out" : "in"} ${record.msg_type}`);

  const meta = el("div", "meta");
  meta.appendChild(el("span", "ty", info.title));
  meta.appendChild(el("span", "", formatTime(record.timestamp)));
  const ack = hl7Ack(info.hl7);
  if (ack) meta.appendChild(el("span", ack.ok ? "ok" : "ko", `${ack.ok ? "✓" : "✕"} ${ack.code}`));
  node.appendChild(meta);

  const bubble = el("div", "bub");
  // ASTM frames stay on one line (as before); other content breaks after <CR>.
  renderTokens(bubble, record.content, {
    breakAfterCr: !record.content.startsWith("<STX>"),
    segments: info.kind === "hl7",
  });
  bubble.title = record.content;
  node.appendChild(bubble);
  return node;
}

function formatTime(value) {
  try {
    return new Date(value).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch (_) {
    return value;
  }
}
