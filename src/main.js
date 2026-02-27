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
  builderProtocol: "#builder-protocol",
  build: "#build-btn",
  builderOutput: "#builder-output",
};

const el = {};
let isConnected = false;

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
  el.build.addEventListener("click", onBuildClick);
  el.builderProtocol.addEventListener("change", () => (el.builderOutput.value = ""));

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
  const protocol = el.protocol.value.toLowerCase();
  const retriesEnabled = el.retries.checked;

  if (!ip || Number.isNaN(port)) {
    return;
  }

  try {
    await invoke("connect_socket", { req: { ip, port, protocol, retries_enabled: retriesEnabled } });
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
  const message = (el.builderOutput.value || el.messageInput.value).trim();
  if (!message) {
    return;
  }

  try {
    await invoke("send_message", { payload: { message } });
    el.builderOutput.value = "";
  } catch (err) {
    console.error("Failed to send", err);
  }
}

function clearLog() {
  el.messageLog.innerHTML = "";
}

function clearInput() {
  el.messageInput.value = "";
  el.builderOutput.value = "";
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
    hl7_message_type: valueOrNull(el.hl7Type.value),
    hl7_response_code: valueOrNull(el.hl7Code.value),
  };

  try {
    await invoke("update_auto_response", { config });
  } catch (err) {
    console.error("Failed to update auto-response", err);
  }
}

async function onBuildClick() {
  const input = el.messageInput.value;
  if (!input.trim()) {
    el.builderOutput.value = "";
    return;
  }

  const protocol = el.builderProtocol.value.toLowerCase();

  try {
    const result = await invoke("build_message_cmd", { req: { protocol, input } });
    el.builderOutput.value = result.output || "";
  } catch (err) {
    console.error("Failed to build message", err);
    el.builderOutput.value = String(err);
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
  const { direction, content, timestamp, auto_response: autoResponse } = payload;
  const entry = document.createElement("div");
  entry.className = `message ${direction === "sent" ? "sent" : "received"}`;

  const time = document.createElement("span");
  time.className = "message-time";
  time.textContent = formatTime(timestamp);
  entry.appendChild(time);

  const body = document.createElement("div");
  body.className = "message-body";
  body.textContent = content;
  if (autoResponse) {
    body.title = "Auto-response";
  }
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
