// Autobuild view: builder form + list of recent autobuilds. All collaborators are injected
// (document, storage, backend, session store) so the real module can be tested with a fake DOM.
import { buildForTarget, deliverToTarget } from "./autobuild-core.js";
import {
  buildKey,
  clearRecentBuilds,
  deleteBuild,
  formatShortTime,
  loadRecentBuilds,
  previewInput,
  recordBuild,
} from "./autobuild-recent-core.js";
import { cloneTemplate } from "../include.js";

const CARD = "autobuild/recent-card.html";
const PREVIEW_LINE = "autobuild/recent-preview-line.html";

/**
 * deps: { doc, storage, invoke, activeId(), hasSession(id), setDraft(id, text),
 *         showSession(target), logError(message, where), now() }
 * `showSession(target)` must show the session view (honouring leave guards) and select target.
 * The recent cards come from cloneTemplate, so the global document must hold the preloaded templates.
 */
export function initAutobuildView(deps) {
  const { doc, storage } = deps;
  const now = deps.now ?? (() => Date.now());
  const $ = (id) => doc.getElementById(id);
  const inputField = $("ab-input");
  const outputField = $("ab-output");
  const noEtbField = $("ab-no-etb");
  const form = $("ab-form");
  const buildButton = $("build-message");
  const errorNode = $("autobuild-error");
  const noteNode = $("ab-note");
  const listNode = $("ab-list");
  const clearButton = $("ab-clear");

  let entries = [];
  let loadButtons = [];
  // Bumped whenever the builder content changes (typing, No ETB, card load); a pending
  // build only writes its output into the builder if nothing changed meanwhile.
  let revision = 0;

  const showError = (message) => {
    errorNode.textContent = message ?? "";
    errorNode.classList.toggle("hidden", !message);
  };

  const showNote = (message) => {
    noteNode.textContent = message ?? "";
    noteNode.classList.toggle("hidden", !message);
  };

  const report = (error, where) => {
    showNote(error);
    if (error) deps.logError(error, where);
  };

  /** Programmatic changes do not fire `change`; config.js needs it to persist data-persist fields. */
  const notifyChange = (field) => {
    if (typeof field.dispatchEvent === "function" && typeof Event === "function") {
      field.dispatchEvent(new Event("change", { bubbles: true }));
    }
  };

  const setFields = ({ input, output, noEtb }) => {
    revision += 1;
    inputField.value = input;
    outputField.value = output;
    noEtbField.checked = noEtb;
    notifyChange(inputField);
    notifyChange(outputField);
    notifyChange(noEtbField);
  };

  // Textareas normalize line breaks to LF, so compare in that form.
  const norm = (s) => String(s).replace(/\r\n?/g, "\n");
  const isLoaded = (entry) =>
    buildKey(norm(entry.input), entry.noEtb) === buildKey(norm(inputField.value), noEtbField.checked);

  function cardFor(entry) {
    const li = cloneTemplate(CARD, { kind: entry.kind, time: formatShortTime(entry.lastUsed, now()) });
    const part = (name) => li.querySelector(`[data-slot="${name}"]`);
    const loaded = isLoaded(entry);
    li.classList.toggle("on", loaded);
    li.dataset.id = entry.id;

    const load = part("load");
    load.setAttribute("aria-pressed", loaded ? "true" : "false");
    if (!entry.noEtb) part("noetb").remove();
    for (const line of previewInput(entry.input)) load.appendChild(cloneTemplate(PREVIEW_LINE, { line }));
    load.addEventListener("click", () => {
      showError(null);
      setFields(entry);
      // Update in place so the clicked button keeps keyboard focus.
      updateSelection();
    });

    const use = part("use");
    use.setAttribute("aria-label", `Use this ${entry.kind} message in the active session`);
    use.addEventListener("click", () => {
      showError(null);
      // The target is captured now: nothing asynchronous can redirect the output.
      const result = deliverToTarget(deps, entry.output);
      if (!result.delivered) return showError(result.error);
      deps.showSession(result.target);
    });

    const del = part("delete");
    del.setAttribute("aria-label", `Delete this ${entry.kind} autobuild from the recent list`);
    del.addEventListener("click", () => {
      const index = entries.findIndex((e) => e.id === entry.id);
      const result = deleteBuild(storage, entry.id);
      entries = result.entries;
      report(result.error, "autobuild-view.js:delete");
      renderList();
      // Next card, else previous, else the builder.
      const target = loadButtons[Math.min(Math.max(index, 0), loadButtons.length - 1)] ?? inputField;
      focusNode(target);
    });
    loadButtons.push(load);
    return li;
  }

  const focusNode = (node) => {
    if (node && typeof node.focus === "function") node.focus();
  };

  /** Refreshes highlight + aria-pressed of the existing cards without recreating them. */
  function updateSelection() {
    const cards = Array.from(listNode.children ?? []);
    cards.forEach((li, i) => {
      const entry = entries[i];
      if (!entry) return;
      const on = isLoaded(entry);
      li.classList.toggle("on", on);
      loadButtons[i]?.setAttribute("aria-pressed", on ? "true" : "false");
    });
  }

  function renderList() {
    listNode.replaceChildren();
    loadButtons = [];
    for (const entry of entries) listNode.appendChild(cardFor(entry));
    listNode.classList.toggle("hidden", entries.length === 0);
    clearButton.disabled = entries.length === 0;
  }

  async function run(deliver) {
    showError(null);
    const input = inputField.value;
    const noEtb = noEtbField.checked;
    if (input.trim() === "") {
      showError("Enter a message to build.");
      return null;
    }
    const startRevision = revision;
    try {
      const result = await buildForTarget(deps, input, noEtb, { deliver });
      if (revision === startRevision) {
        outputField.value = result.output;
        notifyChange(outputField);
      }
      const recorded = recordBuild(storage, { input, output: result.output, noEtb }, now());
      entries = recorded.entries;
      report(recorded.error, "autobuild-view.js:record");
      renderList();
      if (result.error) showError(result.error);
      return result;
    } catch (err) {
      deps.logError(`Autobuild failed: ${err}`, "autobuild-view.js:build");
      showError(`Build failed: ${err}`);
      return null;
    }
  }

  buildButton.addEventListener("click", () => {
    void run(false);
  });

  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    void run(true).then((result) => {
      if (!result || !result.delivered) return;
      deps.showSession(result.target);
    });
  });

  clearButton.addEventListener("click", () => {
    const error = clearRecentBuilds(storage);
    entries = error ? entries : [];
    report(error, "autobuild-view.js:clear");
    renderList();
    focusNode(inputField);
  });

  // The highlighted card always reflects what is currently in the builder.
  inputField.addEventListener("input", () => {
    revision += 1;
    updateSelection();
  });
  noEtbField.addEventListener("change", () => {
    revision += 1;
    updateSelection();
  });

  const loaded = loadRecentBuilds(storage);
  entries = loaded.entries;
  report(loaded.error, "autobuild-view.js:load");
  renderList();

  return { render: renderList, sync: updateSelection, entries: () => entries };
}
