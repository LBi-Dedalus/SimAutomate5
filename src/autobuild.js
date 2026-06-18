const { invoke } = window.__TAURI__.core;

document.addEventListener("DOMContentLoaded", async () => {
  initAutobuildForm();
});

function initAutobuildForm() {
  const autobuildDialog = document.getElementById("autobuild");
  const autobuildForm = autobuildDialog.querySelector("form");
  const autobuildInputTextarea = autobuildForm.input;
  const autobuildOutputTextarea = autobuildForm.output;
  const noETBInput = autobuildForm["no-etb"];
  const buildButton = document.getElementById("build-message");
  const messageTextarea = document.getElementById("message-form").message;

  buildButton.addEventListener("click", async (ev) => {
    const message = autobuildInputTextarea.value;
    const { output } = await invoke("auto_build_message_cmd", {
      req: { input: message, no_etb: noETBInput.checked },
    });
    autobuildOutputTextarea.value = output;
  });

  autobuildForm.addEventListener("submit", async (ev) => {
    const message = autobuildInputTextarea.value;
    const { output } = await invoke("auto_build_message_cmd", {
      req: { input: message, no_etb: noETBInput.checked },
    });
    autobuildOutputTextarea.value = output;
    messageTextarea.value = output;
  });
}
