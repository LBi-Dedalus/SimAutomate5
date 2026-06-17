const { invoke } = window.__TAURI__.core;

window.addEventListener("DOMContentLoaded", init);

function init() {
  initSpecialCharButtons();
  unlockButtonsWhenConnected();
}

function initSpecialCharButtons() {
  const specialCharsDiv = document.getElementById("special-chars");
  const specialCharsButtons = specialCharsDiv.querySelectorAll("button");

  for (const button of specialCharsButtons) {
    button.addEventListener("click", async (ev) => {
      const charToSend = ev.target.textContent;
      const formatted = charToSend
        .split(".")
        .map((ch) => `<${ch.toUpperCase()}>`)
        .join("");

      try {
        console.log("Sending char", formatted);
        await invoke("send_message", { payload: { message: formatted } });
      } catch (err) {
        console.error("Failed to send char", err);
      }
    });
  }
}

function unlockButtonsWhenConnected() {
  window.connection_status.subscribe((status) => {
    const enable = ["connected"].includes(status);

    const specialCharsDiv = document.getElementById("special-chars");
    const specialCharsButtons = specialCharsDiv.querySelectorAll("button");
    for (const button of specialCharsButtons) {
      button.disabled = !enable;
    }
  });
}
