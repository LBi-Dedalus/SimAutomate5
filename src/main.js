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
  openLogs: "#open-logs-btn",
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
const LOG_LEVEL = {
  INF: "INF",
  WRN: "WRN",
  ERR: "ERR",
};

window.addEventListener("DOMContentLoaded", init);

function init() {
  installGlobalErrorLogging();

  Object.entries(selectors).forEach(([key, selector]) => {
    el[key] = document.querySelector(selector);
  });

  el.ip.addEventListener("input", updateConnectEnabled);
  el.port.addEventListener("input", updateConnectEnabled);
  el.connect.addEventListener("click", onConnectClick);
  el.openLogs.addEventListener("click", onOpenLogsClick);
  el.clearLog.addEventListener("click", clearLog);
  el.clearInput.addEventListener("click", clearInput);
  el.send.addEventListener("click", onSendClick);
  el.protocol.addEventListener("change", onProtocolChange);
  el.autoToggle.addEventListener("change", onProtocolChange);
  el.astmMessage.addEventListener("input", onAutoConfigChange);
  el.hl7Type.addEventListener("input", onAutoConfigChange);
  el.hl7Code.addEventListener("input", onAutoConfigChange);
  el.retries.addEventListener("change", persistConfig);
  el.protocol.addEventListener("change", persistConfig);
  el.autobuild.addEventListener("click", onAutobuildClick);

  hydrateConfig();
  applyStatus({ status: "disconnected", attempts: 0, message: null });
  wireEvents();
  updateConnectEnabled();
  updateAutoResponseVisibility();
  onAutoConfigChange();
  persistConfig();
  void logInfo("Application initialized", "main.js:init");
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
  void logInfo("Tauri event listeners registered", "main.js:wireEvents");
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
    void logWarn("Connect skipped due to invalid IP/port", "main.js:connect");
    return;
  }

  try {
    void logInfo(
      `Connect requested (ip=${ip}, port=${port}, retries=${retriesEnabled})`,
      "main.js:connect",
    );
    await invoke("connect_socket", { req: { ip, port, retries_enabled: retriesEnabled } });
  } catch (err) {
    console.error("Failed to connect", err);
    await logError(`Failed to connect: ${String(err)}`, "main.js:connect");
    applyStatus({ status: "error", attempts: 0, message: String(err) });
  }
}

async function disconnect() {
  try {
    void logInfo("Disconnect requested", "main.js:disconnect");
    await invoke("disconnect_socket");
  } catch (err) {
    console.error("Failed to disconnect", err);
    await logError(`Failed to disconnect: ${String(err)}`, "main.js:disconnect");
  }
}

async function onSendClick() {
  const message = el.messageInput.value.trim();
  if (!message) {
    void logWarn("Send skipped because input is empty", "main.js:onSendClick");
    return;
  }

  try {
    void logInfo(`Send requested (length=${message.length})`, "main.js:onSendClick");
    await invoke("send_message", { payload: { message } });
    persistConfig();
  } catch (err) {
    console.error("Failed to send", err);
    await logError(`Failed to send message: ${String(err)}`, "main.js:onSendClick");
  }
}

async function onOpenLogsClick() {
  try {
    await invoke("open_logs_folder");
    void logInfo("Open logs folder requested", "main.js:onOpenLogsClick");
  } catch (err) {
    console.error("Failed to open logs folder", err);
    await logError(`Failed to open logs folder: ${String(err)}`, "main.js:onOpenLogsClick");
  }
}

function clearLog() {
  el.messageLog.innerHTML = "";
  void logInfo("Message log cleared", "main.js:clearLog");
}

function clearInput() {
  el.messageInput.value = "";
  persistConfig();
  void logInfo("Message input cleared", "main.js:clearInput");
}

function onProtocolChange() {
  updateAutoResponseVisibility();
  onAutoConfigChange();
}

function updateAutoResponseVisibility() {
  const protocol = el.protocol.value;

  el.astmConfig.classList.toggle("is-hidden", protocol !== "ASTM");
  el.hl7Config.classList.toggle("is-hidden", protocol !== "HL7");
}

async function onAutoConfigChange() {
  const config = {
    enabled: el.autoToggle.checked,
    astm_message: valueOrNull(el.astmMessage.value),
    protocol: el.protocol.value.toLowerCase(),
    hl7_message_type: valueOrNull(el.hl7Type.value),
    hl7_response_code: valueOrNull(el.hl7Code.value),
  };

  try {
    await invoke("update_auto_response", { config });
  } catch (err) {
    console.error("Failed to update auto-response", err);
    await logError(`Failed to update auto-response: ${String(err)}`, "main.js:onAutoConfigChange");
  }
  
  persistConfig();
}

async function onAutobuildClick() {
  const input = el.messageInput.value.trim();
  if (!input) {
    return;
  }

  try {
    const result = await invoke("auto_build_message_cmd", { req: { input } });
    el.messageInput.value = result.output;
    persistConfig();
    void logInfo(
      `Autobuild completed (input_length=${input.length}, output_length=${result.output.length})`,
      "main.js:onAutobuildClick",
    );
  } catch (err) {
    console.error("Failed to autobuild message", err);
    await logError(`Failed to autobuild message: ${String(err)}`, "main.js:onAutobuildClick");
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
  void logInfo(
    `Status updated (status=${status}, attempts=${attempts}, has_message=${Boolean(message)})`,
    "main.js:applyStatus",
  );
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
  void logInfo(
    `Message appended (direction=${direction}, length=${content.length}, timestamp=${timestamp})`,
    "main.js:appendMessage",
  );
}

function formatTime(value) {
  try {
    return new Date(value).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      fractionalSecondDigits: 3 
    });
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
    void logError(`Failed to persist config: ${String(err)}`, "main.js:persistConfig");
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
    void logError(`Failed to hydrate config: ${String(err)}`, "main.js:hydrateConfig");
  }
}

function installGlobalErrorLogging() {
  window.addEventListener("error", (event) => {
    const location = event.filename
      ? `${event.filename.split("/").at(-1)}:${event.lineno || 0}`
      : "main.js:window.onerror";
    void logError(`Unhandled error: ${event.message}`, location);
  });

  window.addEventListener("unhandledrejection", (event) => {
    const reason = event.reason instanceof Error ? event.reason.message : String(event.reason);
    void logError(`Unhandled rejection: ${reason}`, "main.js:unhandledrejection");
  });
}

function deriveLocation(fallback) {
  const stack = new Error().stack;
  if (!stack) return fallback;

  const lines = stack.split("\n");
  const candidate = lines.find((line) => line.includes("main.js"));
  if (!candidate) return fallback;

  const match = candidate.match(/main\.js:(\d+):\d+/);
  if (!match) return fallback;

  return `main.js:${match[1]}`;
}

async function writeFrontendLog(level, message, location) {
  const entry = {
    level,
    location: location || deriveLocation("main.js:unknown"),
    message,
  };

  try {
    await invoke("log_frontend", { entry });
  } catch (err) {
    console.error("Failed to write frontend log", err);
  }
}

function logInfo(message, location) {
  return writeFrontendLog(LOG_LEVEL.INF, message, location);
}

function logWarn(message, location) {
  return writeFrontendLog(LOG_LEVEL.WRN, message, location);
}

function logError(message, location) {
  return writeFrontendLog(LOG_LEVEL.ERR, message, location);
}
