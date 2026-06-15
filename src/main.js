const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const selectors = {};

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
  // initModeButtons();
}
