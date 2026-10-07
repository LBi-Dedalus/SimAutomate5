// Frontend logging through the backend log files (frontend.log).
// Never pass raw protocol payloads here: traffic logging is metadata-only.

function send(level, message, location) {
  try {
    const invoke = window.__TAURI__?.core?.invoke;
    if (!invoke) return;
    invoke("log_frontend", { entry: { level, location, message } }).catch(
      () => {},
    );
  } catch (_) {
    // Logging must never break the UI.
  }
}

export function logInfo(message, location = "frontend") {
  send("INF", message, location);
}

export function logWarn(message, location = "frontend") {
  send("WRN", message, location);
}

export function logError(message, location = "frontend") {
  send("ERR", message, location);
}
