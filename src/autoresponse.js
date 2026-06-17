const { invoke } = window.__TAURI__.core;

window.addEventListener("DOMContentLoaded", init);

async function init() {
  await updateAutoResponseConfig();
  initAutoResponseSwitch();
  initAutoResponseDialog();
}

async function updateAutoResponseConfig() {
  const config = getAutoResponseConfig();
  await invoke("update_auto_response", { config });
}

function getAutoResponseConfig() {
  const autoResponseDialog = document.getElementById("autoresponse");
  const autoResponseForm = autoResponseDialog.querySelector("form");

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

  autoResponseSwitch.addEventListener("change", async (ev) => {
    await updateAutoResponseConfig();
  });
}

function initAutoResponseDialog() {
  const autoResponseDialog = document.getElementById("autoresponse");
  const autoResponseForm = autoResponseDialog.querySelector("form");

  autoResponseForm.addEventListener("submit", async (ev) => {
    await updateAutoResponseConfig();
  });
}
