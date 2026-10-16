import { showView } from "../nav.js";
import { store } from "../session/sessions.js";
import { setComposerText } from "../session/messages.js";
import { initAutobuildView } from "./autobuild-view.js";
import { logError } from "../log.js";

const { invoke } = window.__TAURI__.core;

// config.js has already hydrated the persisted fields: it is imported before this script.
const view = initAutobuildView({
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
view.sync();
