import { showView } from "./nav.js";
import { store } from "./sessions.js";
import { setComposerText } from "./messages.js";
import { buildForTarget } from "./autobuild-core.js";
import { logError } from "./log.js";

const { invoke } = window.__TAURI__.core;

document.addEventListener("DOMContentLoaded", () => {
  initAutobuildForm();
});

function initAutobuildForm() {
  const dialog = document.getElementById("autobuild");
  const form = dialog.querySelector("form");
  const buildButton = document.getElementById("build-message");
  const errorNode = document.getElementById("autobuild-error");

  const deps = {
    invoke,
    activeId: () => store.activeId,
    hasSession: (id) => !!store.get(id),
    setDraft: (id, text) => setComposerText(text, id),
  };

  const showError = (message) => {
    if (!errorNode) return;
    errorNode.textContent = message ?? "";
    errorNode.classList.toggle("hidden", !message);
  };

  const run = async (deliver) => {
    showError(null);
    try {
      const result = await buildForTarget(deps, form.input.value, form["no-etb"].checked, { deliver });
      form.output.value = result.output;
      if (result.error) showError(result.error);
      return result;
    } catch (err) {
      logError(`Autobuild failed: ${err}`, "autobuild.js:build");
      showError(`Build failed: ${err}`);
      return null;
    }
  };

  buildButton.addEventListener("click", () => {
    void run(false);
  });

  form.addEventListener("submit", (ev) => {
    // The dialog stays open until the async build has really delivered the text.
    ev.preventDefault();
    void run(true).then((result) => {
      if (!result || !result.delivered) return;
      dialog.close();
      // Show the session that received the text, honouring the template-editor guard.
      showView("session", { onActivate: () => store.select(result.target) });
    });
  });
}
