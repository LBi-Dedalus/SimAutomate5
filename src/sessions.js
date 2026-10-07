// The application's session controller, wired to Tauri. Only this module listens to the
// backend events, so every status/message is routed by its session id and attempt.
import { createSessionController } from "./sessions-controller.js";
import { logError, logWarn } from "./log.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const STATUS_EVENT = "connection://status";
const MESSAGE_EVENT = "message://stream";

export const sessions = createSessionController({
  invoke: (...args) => invoke(...args),
  // Resolved lazily: the storage is only needed when the first session is created.
  storage: {
    getItem: (key) => localStorage.getItem(key),
    setItem: (key, value) => localStorage.setItem(key, value),
  },
  log: { error: logError, warn: logWarn },
});

export const store = sessions.store;

/** Status of the selected session, as seen by the modules that only care about "the" connection. */
export function syncActiveStatus() {
  const active = store.active();
  window.connection_status?.set(active ? active.status : "disconnected");
}

store.subscribe((event) => {
  if (["status", "select", "removed", "created"].includes(event.type)) syncActiveStatus();
});

/**
 * Single send-eligibility signal for every send surface: calls `callback(target)` now and
 * whenever status, operation (disconnecting/closing), selection or the session list change.
 * `target` is sessions.sendTarget(): null means sending is not allowed.
 */
export function subscribeSendEligibility(callback) {
  const refresh = () => callback(sessions.sendTarget());
  store.subscribe((event) => {
    if (["status", "op", "select", "removed", "created"].includes(event.type)) refresh();
  });
  refresh();
}

window.addEventListener("DOMContentLoaded", async () => {
  await listen(STATUS_EVENT, (event) => sessions.handleStatus(event.payload));
  await listen(MESSAGE_EVENT, (event) => sessions.handleMessage(event.payload));
});
