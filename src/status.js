const { listen } = window.__TAURI__.event;

const STATUS_EVENT = "connection://status";

const STATUS_LABELS = {
    disconnected: "Disconnected",
    connecting: "Connecting",
    connected: "Connected",
    error: "Error",
};

const STATUS_CLASSES = {
    disconnected: "badge outline",
    connecting: "badge",
    connected: "badge",
    error: "badge",
};

const STATUS_VARIANTS = {
    disconnected: "",
    connecting: "warning",
    connected: "success",
    error: "danger",
};

window.addEventListener("DOMContentLoaded", async () => {
    await listen(STATUS_EVENT, (event) => applyStatus(event.payload));
});

function applyStatus(payload) {
    const { status, attempts, message } = payload;
    const label = statusLabel(status, attempts, message);
    const statusEl = document.getElementById("status");

    statusEl.textContent = label;
    statusEl.className = STATUS_CLASSES[status];
    statusEl.dataset.variant = STATUS_VARIANTS[status];

    window.isConnected.set(status === "connected");
}

function statusLabel(status, attempts, message) {
    const base = STATUS_LABELS[status] || "Unknown";

    const attemptText = status === "connecting" && attempts > 1 ? ` (try ${attempts})` : "";
    const detail = message ? ` — ${message}` : "";
    return `⬤ ${base}${attemptText}${detail}`;
}