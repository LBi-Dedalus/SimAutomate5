import { showView } from "./nav.js";
import { store } from "./sessions.js";
import { setComposerText } from "./messages.js";
import { initAutobuildView } from "./autobuild-view.js";
import { logError } from "./log.js";

const { invoke } = window.__TAURI__.core;

let view = null;

// config.js hydrates the persisted fields on the *window* DOMContentLoaded, which runs after
// this document-level listener. Re-sync the selected card once everything has been hydrated:
// window "load" fires after all DOMContentLoaded handlers, regardless of registration order.
window.addEventListener("load", () => view?.sync());

document.addEventListener("DOMContentLoaded", () => {
  view = initAutobuildView({
    doc: document,
    storage: localStorage,
    invoke,
    activeId: () => store.activeId,
    hasSession: (id) => !!store.get(id),
    setDraft: (id, text) => setComposerText(text, id),
    // Show the session that received the text, honouring the template-editor guard.
    showSession: (target) => showView("session", { onActivate: () => store.select(target) }),
    logError,
  });
});
