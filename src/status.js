const { listen } = window.__TAURI__.event;

const STATUS_EVENT = "connection://status";

const STATUS_LABELS = {
  disconnected: "Disconnected",
  connecting: "Connecting",
  connected: "Connected",
  error: "Error",
};

const STATUS_CLASSES = {
  disconnected: "outline",
  connecting: "warning",
  connected: "success",
  error: "danger",
};

const STATUS_VARIANTS = {
  disconnected: "",
  connecting: "warning",
  connected: "success",
  error: "danger",
};

window.connection_status = {
  _value: "disconnected",
  _subscribers: new Set(),
  get() {
    return this._value;
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
  const statusEl = document.getElementById("status");

  statusEl.textContent = STATUS_LABELS[status];
  statusEl.className = "badge text-small " + STATUS_CLASSES[status];
  statusEl.dataset.variant = STATUS_VARIANTS[status];

  window.connection_status.set(status);
}
