import { tokenPayload } from "../control-chars.js";
import { sendMessage, activeSendTarget } from "./messages.js";
import { logError } from "../log.js";
import { subscribeSendEligibility } from "./sessions.js";

function init() {
  initSpecialCharButtons();
  unlockButtonsWhenConnected();
}

function initSpecialCharButtons() {
  window.actions ??= {};
  window.actions.chars = {
    async send(_ev, button) {
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
      // Target captured at click time.
      await sendMessage(formatted, activeSendTarget());
    },
  };
}

function unlockButtonsWhenConnected() {
  subscribeSendEligibility((target) => {
    const enable = Boolean(target);

    const specialCharsDiv = document.getElementById("special-chars");
    const specialCharsButtons = specialCharsDiv.querySelectorAll("button");
    for (const button of specialCharsButtons) {
      button.disabled = !enable;
    }
  });
}

init();
