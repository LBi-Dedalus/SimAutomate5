import { tokenPayload } from "./control-chars.js";
import { sendMessage } from "./messages.js";
import { logError } from "./log.js";

window.addEventListener("DOMContentLoaded", init);

function init() {
  initSpecialCharButtons();
  unlockButtonsWhenConnected();
}

function initSpecialCharButtons() {
  const specialCharsDiv = document.getElementById("special-chars");
  const specialCharsButtons = specialCharsDiv.querySelectorAll("button");

  for (const button of specialCharsButtons) {
    button.addEventListener("click", async () => {
      // The payload comes from data-token, never from the (decorated) label text.
      const formatted = tokenPayload(button.dataset.token);
      if (formatted === null) {
        console.error("Unknown control token", button.dataset.token);
        logError(
          `Unknown control token on button: ${button.dataset.token}`,
          "special-chars.js:click",
        );
        return;
      }
      console.log("Sending char", formatted);
      await sendMessage(formatted);
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
