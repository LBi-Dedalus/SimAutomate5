const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const selectors = {
};

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
  initModeButtons();
}

function initModeButtons() {
  const clientModeBtn = document.getElementById("client-mode");
  const serverModeBtn = document.getElementById("server-mode");
  const clientOptions = document.getElementById("client-options");
  const serverOptions = document.getElementById("server-options");

  clientModeBtn.addEventListener("click", () => {
    const disable = ["connecting", "connected"].includes(window.status.get());
    if (disable) return;

    clientModeBtn.classList.remove("ghost");
    serverModeBtn.classList.add("ghost");
    clientOptions.classList.remove("hidden");
    serverOptions.classList.add("hidden");
  });

  serverModeBtn.addEventListener("click", () => {
    const disable = ["connecting", "connected"].includes(window.status.get());
    if (disable) return;

    serverModeBtn.classList.remove("ghost");
    clientModeBtn.classList.add("ghost");
    serverOptions.classList.remove("hidden");
    clientOptions.classList.add("hidden");
  });
}
