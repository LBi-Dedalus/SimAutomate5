const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const selectors = {
  ip: "#ip-address",
  port: "#port",
  retries: "#enable-retries",
  protocol: "#protocol",
  connect: "#connect-btn",
  status: "#conn-status",
  statusLabel: "#conn-label",
  messageLog: "#message-log",
  clearLog: "#clear-log-btn",
  messageInput: "#message-input",
  clearInput: "#clear-input-btn",
  send: "#send-btn",
  autoToggle: "#auto-response",
  astmConfig: "#astm-response-config",
  astmMessage: "#astm-response",
  hl7Config: "#hl7-response-config",
  hl7Type: "#hl7-msg-type",
  hl7Code: "#hl7-resp-code",
  autobuild: "#autobuild-btn",
};

const el = {};
let isConnected = false;
const CONFIG_KEY = "simautomate:config";

const STATUS_EVENT = "connection://status";
const MESSAGE_EVENT = "message://stream";

window.addEventListener("DOMContentLoaded", init);

function init() {
  Object.entries(selectors).forEach(([key, selector]) => {
    el[key] = document.querySelector(selector);
  });

  el.ip.addEventListener("input", updateConnectEnabled);
  el.port.addEventListener("input", updateConnectEnabled);
  el.connect.addEventListener("click", onConnectClick);
  el.clearLog.addEventListener("click", clearLog);
  el.clearInput.addEventListener("click", clearInput);
  el.send.addEventListener("click", onSendClick);
  el.protocol.addEventListener("change", onProtocolChange);
  el.autoToggle.addEventListener("change", onAutoConfigChange);
  el.astmMessage.addEventListener("input", onAutoConfigChange);
  el.hl7Type.addEventListener("input", onAutoConfigChange);
  el.hl7Code.addEventListener("input", onAutoConfigChange);
  el.retries.addEventListener("change", persistConfig);
  el.protocol.addEventListener("change", persistConfig);
  el.autobuild.addEventListener("click", onAutobuildClick);

  hydrateConfig();
  applyStatus({ status: "disconnected", attempts: 0, message: null });
  updateConnectEnabled();
  updateAutoResponseVisibility();
  onAutoConfigChange();
  wireEvents();
}

function updateConnectEnabled() {
  const ipReady = el.ip.value.trim().length > 0;
  const portReady = el.port.value.trim().length > 0;
  el.connect.disabled = !(ipReady && portReady);
  persistConfig();
}

async function wireEvents() {
  await listen(STATUS_EVENT, (event) => applyStatus(event.payload));
  await listen(MESSAGE_EVENT, (event) => appendMessage(event.payload));
}

async function onConnectClick() {
  if (isConnected) {
    await disconnect();
    return;
  }
  await connect();
}

async function connect() {
  const ip = el.ip.value.trim();
  const port = Number.parseInt(el.port.value.trim(), 10);
  const retriesEnabled = el.retries.checked;

  persistConfig();

  if (!ip || Number.isNaN(port)) {
    return;
  }

  try {
    await invoke("connect_socket", { req: { ip, port, retries_enabled: retriesEnabled } });
  } catch (err) {
    console.error("Failed to connect", err);
    applyStatus({ status: "error", attempts: 0, message: String(err) });
  }
}

async function disconnect() {
  try {
    await invoke("disconnect_socket");
  } catch (err) {
    console.error("Failed to disconnect", err);
  }
}

async function onSendClick() {
  const message = el.messageInput.value.trim();
  if (!message) {
    return;
  }

  try {
    await invoke("send_message", { payload: { message } });
    el.messageInput.value = "";
    persistConfig();
  } catch (err) {
    console.error("Failed to send", err);
  }
}

function clearLog() {
  el.messageLog.innerHTML = "";
}

function clearInput() {
  el.messageInput.value = "";
  persistConfig();
}

function onProtocolChange() {
  updateAutoResponseVisibility();
  onAutoConfigChange();
}

function updateAutoResponseVisibility() {
  const enabled = el.autoToggle.checked;
  const protocol = el.protocol.value;
  const showAstm = enabled && protocol === "ASTM";
  const showHl7 = enabled && protocol === "HL7";

  el.astmConfig.hidden = !showAstm;
  el.hl7Config.hidden = !showHl7;
}

async function onAutoConfigChange() {
  updateAutoResponseVisibility();
  const config = {
    enabled: el.autoToggle.checked,
    astm_message: valueOrNull(el.astmMessage.value),
    protocol: el.protocol.value,
    hl7_message_type: valueOrNull(el.hl7Type.value),
    hl7_response_code: valueOrNull(el.hl7Code.value),
  };

  try {
    await invoke("update_auto_response", { config });
  } catch (err) {
    console.error("Failed to update auto-response", err);
  }

  persistConfig();
}

async function onAutobuildClick() {
  const input = el.messageInput.value;
  if (!input.trim()) {
    return;
  }

  try {
    const result = await invoke("auto_build_message_cmd", { req: { input } });
    el.messageInput.value = result.output;
    persistConfig();
  } catch (err) {
    console.error("Failed to autobuild message", err);
  }
}

function applyStatus(payload) {
  const { status, attempts, message } = payload;
  const label = statusLabel(status, attempts, message);
  const statusEl = el.status;

  statusEl.classList.remove("connected", "connecting", "error");
  if (status === "connected") statusEl.classList.add("connected");
  if (status === "connecting") statusEl.classList.add("connecting");
  if (status === "error") statusEl.classList.add("error");

  el.statusLabel.textContent = label;
  isConnected = status === "connected";
  el.connect.textContent = isConnected ? "Disconnect" : "Connect";
  el.send.disabled = !isConnected;
}

function statusLabel(status, attempts, message) {
  const base = {
    disconnected: "Disconnected",
    connecting: "Connecting",
    connected: "Connected",
    error: "Error",
  }[status] || "Unknown";

  const attemptText = status === "connecting" && attempts > 1 ? ` (try ${attempts})` : "";
  const detail = message ? ` — ${message}` : "";
  return `${base}${attemptText}${detail}`;
}

function appendMessage(payload) {
  const { direction, content, timestamp } = payload;
  const entry = document.createElement("div");
  entry.className = `message ${direction === "sent" ? "sent" : "received"}`;

  const time = document.createElement("span");
  time.className = "message-time";
  time.textContent = formatTime(timestamp);
  entry.appendChild(time);

  const body = document.createElement("div");
  body.className = "message-body";
  body.textContent = content;
  entry.appendChild(body);

  el.messageLog.appendChild(entry);
  el.messageLog.scrollTop = el.messageLog.scrollHeight;
}

function formatTime(value) {
  try {
    return new Date(value).toLocaleTimeString();
  } catch (_) {
    return value;
  }
}

function valueOrNull(raw) {
  const trimmed = raw.trim();
  return trimmed.length ? trimmed : null;
}

function persistConfig() {
  const data = {
    ip: el.ip.value,
    port: el.port.value,
    retries: el.retries.checked,
    protocol: el.protocol.value,
    auto: el.autoToggle.checked,
    astm: el.astmMessage.value,
    hl7Type: el.hl7Type.value,
    hl7Code: el.hl7Code.value,
    message: el.messageInput.value,
  };

  try {
    localStorage.setItem(CONFIG_KEY, JSON.stringify(data));
  } catch (err) {
    console.error("Failed to persist config", err);
  }
}

function hydrateConfig() {
  try {
    const raw = localStorage.getItem(CONFIG_KEY);
    if (!raw) return;
    const data = JSON.parse(raw);
    if (typeof data.ip === "string") el.ip.value = data.ip;
    if (typeof data.port === "string") el.port.value = data.port;
    if (typeof data.retries === "boolean") el.retries.checked = data.retries;
    if (typeof data.protocol === "string") el.protocol.value = data.protocol;
    if (typeof data.auto === "boolean") el.autoToggle.checked = data.auto;
    if (typeof data.astm === "string") el.astmMessage.value = data.astm;
    if (typeof data.hl7Type === "string") el.hl7Type.value = data.hl7Type;
    if (typeof data.hl7Code === "string") el.hl7Code.value = data.hl7Code;
    if (typeof data.message === "string") el.messageInput.value = data.message;
  } catch (err) {
    console.error("Failed to hydrate config", err);
  }
}
