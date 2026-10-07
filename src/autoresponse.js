import { logError } from "./log.js";

const { invoke } = window.__TAURI__.core;

window.addEventListener("DOMContentLoaded", init);

async function init() {
  initAutoResponseSwitch();
  initAutoResponseForm();
  await applyAutoResponseConfig();
}

async function updateAutoResponseConfig() {
  const config = getAutoResponseConfig();
  await invoke("update_auto_response", { config });
  document
    .getElementById("autoreply-badge")
    .classList.toggle("hidden", !config.enabled);
}

// Pushes the config to the backend and reports failures visibly.
async function applyAutoResponseConfig(successText = "") {
  const status = document.getElementById("autoresponse-status");
  try {
    await updateAutoResponseConfig();
    status.textContent = successText;
    status.classList.remove("inline-error");
  } catch (err) {
    console.error("Failed to update auto response", err);
    logError(
      `Failed to update auto response: ${String(err)}`,
      "autoresponse.js:apply",
    );
    status.textContent = `Could not apply auto reply settings: ${String(err)}`;
    status.classList.add("inline-error");
  }
}

function getAutoResponseConfig() {
  const autoResponseForm = document.getElementById("autoresponse-form");

  const enabled = document.getElementById("autoresponse-activate").checked;
  const astmMessage = autoResponseForm["astm_ack"].value.trim() || null;
  const hl7Type = autoResponseForm["hl7_type"].value.trim() || null;
  const hl7Code = autoResponseForm["hl7_code"].value.trim() || null;

  return {
    enabled: enabled,
    astm_message: astmMessage,
    hl7_message_type: hl7Type,
    hl7_response_code: hl7Code,
  };
}

function initAutoResponseSwitch() {
  const autoResponseSwitch = document.getElementById("autoresponse-activate");

  autoResponseSwitch.addEventListener("change", async () => {
    await applyAutoResponseConfig("Applied.");
  });
}

function initAutoResponseForm() {
  const autoResponseForm = document.getElementById("autoresponse-form");

  autoResponseForm.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    await applyAutoResponseConfig("Saved and applied.");
  });
}
