// The rename box shared by the sidebar sessions, the sidebar recent items and the Home cards:
// one action, one shared connection name (saved name + every linked open session).
import { sessions } from "./sessions.js";
import { describeEndpoint, loadNames, savedName } from "../home/recent-core.js";

const $ = (id) => document.getElementById(id);

/** Endpoint being renamed while the box is open; null when closed. */
let target = null;

function showError(message) {
  const node = $("rename-error");
  node.textContent = message ?? "";
  node.classList.toggle("hidden", !message);
}

/** Opens the rename box for a connection (its normalized or dataset-shaped endpoint). */
export function requestRename(endpoint) {
  const dialog = $("rename-dialog");
  if (!dialog || dialog.open) return;
  target = endpoint;
  const { names } = loadNames(localStorage);
  const input = $("rename-input");
  input.value = savedName(names, endpoint);
  $("rename-address").textContent = `Address: ${describeEndpoint(endpoint)}. Leave empty to use the address.`;
  showError(null);
  dialog.addEventListener("close", () => (target = null), { once: true });
  dialog.showModal();
  input.focus();
  input.select?.();
}

function save(ev) {
  ev.preventDefault();
  if (!target) return;
  const result = sessions.renameConnection(target, $("rename-input").value);
  if (!result.ok) {
    // Stay open: success is never reported when the name was not saved.
    showError(result.error);
    return;
  }
  $("rename-dialog").close("saved");
}

window.actions ??= {};
window.actions.rename = {
  save,
  cancel: () => $("rename-dialog").close("cancel"),
};
