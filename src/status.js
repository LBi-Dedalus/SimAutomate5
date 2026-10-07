const { listen } = window.__TAURI__.event;

const STATUS_EVENT = "connection://status";

const STATUS_LABELS = {
  disconnected: "Disconnected",
  connecting: "Connecting",
  listening: "Listening",
  connected: "Connected",
  error: "Error",
};

// Pill modifier per status; "connected" uses the default (green) pill.
const STATUS_PILL_CLASSES = {
  disconnected: "off",
  connecting: "wait",
  listening: "wait",
  connected: "",
  error: "err",
};

// A connection attempt, a listening server or a live connection owns the forms.
const BUSY_STATUSES = ["connecting", "listening", "connected"];

window.connection_status = {
  _value: "disconnected",
  _subscribers: new Set(),
  get() {
    return this._value;
  },
  isBusy() {
    return BUSY_STATUSES.includes(this._value);
  },
  set(val) {
    this._value = val;
    for (const callback of this._subscribers) {
      callback(val);
    }
  },
  subscribe(callback) {
    this._subscribers.add(callback);
    return () => this._subscribers.delete(callback);
  },
};

window.addEventListener("DOMContentLoaded", async () => {
  await listen(STATUS_EVENT, (event) => applyStatus(event.payload));
});

function applyStatus(payload) {
  const { status } = payload;
  const label = STATUS_LABELS[status] ?? status;
  const cls = STATUS_PILL_CLASSES[status] ?? "off";

  for (const pill of document.querySelectorAll("[data-status-pill]")) {
    pill.className = cls ? `pill ${cls}` : "pill";
    pill.replaceChildren(document.createElement("i"), document.createTextNode(label));
  }

  window.connection_status.set(status);
}
