import { describeMessage, hl7Ack } from "./inspector-core.js";
import { showInspector, clearInspector } from "./inspector.js";
import { renderTokens, el } from "./render.js";
import { logError } from "./log.js";

const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const MESSAGE_EVENT = "message://stream";
const MAX_MESSAGES = 2000;

let sequence = 0;
/** @type {{record: object, node: HTMLElement}[]} */
const entries = [];
let selectedId = null;

document.addEventListener("DOMContentLoaded", async () => {
  initMessageForm();
  await initChat();
  unlockMessageInputWhenConnected();
});

/** The single send path used by the composer, control buttons and templates. */
export async function sendMessage(message) {
  try {
    console.log("Sending message", message);
    await invoke("send_message", { payload: { message } });
    return true;
  } catch (err) {
    console.error("Failed to send message", err);
    logError(`Failed to send message: ${String(err)}`, "messages.js:sendMessage");
    return false;
  }
}

/** Puts text in the composer (and lets the persistence layer see the change). */
export function setComposerText(text) {
  const textarea = document.getElementById("message-form").message;
  textarea.value = text;
  textarea.dispatchEvent(new Event("change", { bubbles: true }));
}

function initMessageForm() {
  const messageForm = document.getElementById("message-form");
  messageForm.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    await sendMessage(messageForm.message.value);
  });
}

async function initChat() {
  document.getElementById("clear-chat").addEventListener("click", clearMessages);
  await listen(MESSAGE_EVENT, (event) => appendMessage(event.payload));
}

function clearMessages() {
  for (const { node } of entries) node.remove();
  entries.length = 0;
  selectedId = null;
  clearInspector();
  updateStats();
  document.getElementById("no-connection").classList.remove("hidden");
}

function unlockMessageInputWhenConnected() {
  window.connection_status.subscribe((status) => {
    const enable = ["connected"].includes(status);

    const messageForm = document.getElementById("message-form");
    const sendButton = messageForm.querySelector('button[type="submit"]');
    if (sendButton) {
      sendButton.disabled = !enable;
    }
  });
}

function select(id) {
  const entry = entries.find((item) => item.record.id === id);
  if (!entry) return;
  selectedId = id;
  for (const item of entries) {
    item.node.classList.toggle("sel", item.record.id === id);
  }
  showInspector(entry.record);
}

function appendMessage(payload) {
  const record = { id: ++sequence, ...payload };
  const info = describeMessage(record);
  const node =
    info.kind === "system" ? buildSystemNode(record) : buildBubbleNode(record, info);

  node.tabIndex = 0;
  node.addEventListener("click", () => select(record.id));
  node.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" || ev.key === " ") {
      ev.preventDefault();
      select(record.id);
    }
  });

  const messagesEl = document.getElementById("messages");
  document.getElementById("no-connection").classList.add("hidden");

  messagesEl.appendChild(node);
  entries.push({ record, node });

  while (entries.length > MAX_MESSAGES) {
    const removed = entries.shift();
    removed.node.remove();
    if (removed.record.id === selectedId) {
      selectedId = null;
      clearInspector();
    }
  }
  updateStats();
  messagesEl.scrollTop = messagesEl.scrollHeight;
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

function updateStats() {
  const stats = document.getElementById("session-stats");
  let sent = 0;
  let received = 0;
  for (const { record } of entries) {
    if (record.msg_type === "sent") sent++;
    else if (record.msg_type === "received") received++;
  }
  stats.textContent = `↑ ${sent} sent · ↓ ${received} received`;
  stats.classList.toggle("hidden", sent + received === 0);
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
