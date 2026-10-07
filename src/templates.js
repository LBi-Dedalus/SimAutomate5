// Templates view: compact list + always-docked editor with CRUD, variables, preview,
// load-in-composer and send. Persistence goes through the backend config file
// (load_templates / save_templates); nothing is stored in localStorage.
import {
  extractVariables,
  syncVariables,
  manualValues,
  resolveTemplate,
  makeContext,
  snapshot,
  newTemplateId,
  isAutoVariable,
} from "./template-core.js";
import { tokenPayload, splitTokens } from "./control-chars.js";
import { describeMessage, parseHl7, stripHl7Framing } from "./inspector-core.js";
import { showView, setLeaveGuard, currentView } from "./nav.js";
import { sendMessage, setComposerText } from "./messages.js";
import { SAVE_AS_TEMPLATE_EVENT } from "./inspector.js";
import { el, renderTokens } from "./render.js";
import { logError, logInfo } from "./log.js";

const { invoke } = window.__TAURI__.core;

/** Templates as persisted in config.json. */
let templates = [];
let loadError = null;
/** True only after the persisted collection was read successfully (an empty [] is a real, loaded value). */
let loaded = false;
let loading = false;
let loadPromise = null;

/** The template shown in the editor (a working copy). */
let draft = null;
/** Snapshot of the draft when opened/saved; null means "never saved, always dirty". */
let baseline = null;
let ctx = makeContext();
let saving = false;
let pendingAction = null;
let deleteArmed = false;
let deleteTimer = null;

const $ = (id) => document.getElementById(id);

window.addEventListener("DOMContentLoaded", init);

async function init() {
  setLeaveGuard("templates", (proceed) => {
    if (saving) {
      showBanner("A save is in progress. Try again in a moment.", "ok");
      return false;
    }
    if (!isDirty()) return true;
    askBeforeLeaving(proceed);
    return false;
  });

  $("tpl-new").addEventListener("click", () => requestAction(newTemplate));
  $("tpl-new-card").addEventListener("click", () => requestAction(newTemplate));
  $("tpl-search").addEventListener("input", renderList);
  $("tpl-name").addEventListener("input", onNameInput);
  $("tpl-desc").addEventListener("input", onDescInput);
  $("tpl-payload").addEventListener("input", onPayloadInput);
  for (const type of ["keyup", "click", "focus", "blur"]) {
    $("tpl-payload").addEventListener(type, updateCursor);
  }
  $("tpl-save").addEventListener("click", () => void save());
  $("tpl-delete").addEventListener("click", () => void onDelete());
  $("tpl-load").addEventListener("click", loadInComposer);
  $("tpl-send").addEventListener("click", () => void sendNow());
  $("tpl-insert-var").addEventListener("click", insertVariable);
  $("tpl-banner-close").addEventListener("click", hideBanner);
  $("tpl-banner-retry").addEventListener("click", () => void loadAll());
  $("tpl-guard-save").addEventListener("click", () => void guardSave());
  $("tpl-guard-discard").addEventListener("click", guardDiscard);
  $("tpl-guard-cancel").addEventListener("click", guardCancel);

  for (const button of document.querySelectorAll("#tpl-form [data-insert]")) {
    button.addEventListener("click", () => {
      const text = tokenPayload(button.dataset.insert);
      if (text !== null) insertAtCursor(text);
    });
  }

  document.addEventListener(SAVE_AS_TEMPLATE_EVENT, (ev) =>
    requestAction(() => newTemplate(ev.detail.payload), { reveal: true }),
  );

  window.addEventListener("beforeunload", (ev) => {
    if (isDirty()) {
      ev.preventDefault();
      ev.returnValue = "";
    }
  });

  window.connection_status.subscribe(updateSendEnabled);
  updateSendEnabled();

  await loadAll();
}

// ── Loading ─────────────────────────────────────────────────

/**
 * Loads are serialized (an in-flight load is shared, never overlapped) and writes are
 * refused while one is pending or after one failed: `loaded` is true only once the
 * persisted collection was read, so an unknown collection can never be overwritten.
 * Resolves true when the collection is loaded.
 */
function loadAll() {
  if (saving) return Promise.resolve(loaded); // never replace the list under a pending write
  if (!loadPromise) {
    loadPromise = doLoad().finally(() => {
      loadPromise = null;
    });
  }
  return loadPromise;
}

async function doLoad() {
  loading = true;
  renderAll();
  let ok = false;
  try {
    const result = await invoke("load_templates");
    templates = result.templates;
    loadError = null;
    loaded = true;
    ok = true;
    if (result.seeded) logInfo("Built-in templates provided (no saved templates yet)", "templates.js:load");
  } catch (err) {
    templates = [];
    loadError = String(err);
    loaded = false;
    logError(`Failed to load templates: ${loadError}`, "templates.js:load");
  } finally {
    loading = false;
  }
  if (ok) hideBanner();
  else showBanner(`Could not load templates: ${loadError}. Saving is disabled so the file is not overwritten.`, "error", { retry: true });
  if (draft && !isDirty() && !(ok && templates.some((t) => t.id === draft.id))) draft = null;
  renderAll();
  return ok;
}

// ── State helpers ───────────────────────────────────────────

function isDirty() {
  if (!draft) return false;
  return baseline === null || snapshot(draft) !== baseline;
}

function cloneForEditor(template) {
  return {
    id: template.id,
    name: template.name,
    description: template.description ?? "",
    payload: template.payload,
    variables: syncVariables(template.payload, template.variables ?? []),
    isNew: false,
  };
}

function toPersisted(d) {
  return {
    id: d.id,
    name: d.name.trim(),
    description: d.description,
    payload: d.payload,
    variables: syncVariables(d.payload, d.variables).map((v) => ({
      name: v.name,
      default: v.default,
    })),
  };
}

/** Persisted-collection operations (new/select/save/delete) need a loaded, idle collection. */
function editingAllowed() {
  return loaded && !loading;
}

function unavailableMessage(what) {
  return loading
    ? `Templates are still loading: ${what} is unavailable for a moment.`
    : `${what[0].toUpperCase()}${what.slice(1)} is disabled: the templates could not be loaded${loadError ? ` (${loadError})` : ""}.`;
}

// ── Dirty guard ─────────────────────────────────────────────

/** Runs `action` now, or after the user decided what to do with unsaved edits. */
function requestAction(action, { reveal = false } = {}) {
  if (!editingAllowed()) {
    // Not loaded yet / load failed: nothing may touch the draft or collection.
    if (reveal && currentView() !== "templates") showView("templates", { force: true });
    // Explicit refusal: requests are never queued, the user repeats the action.
    showBanner(
      loading
        ? "Templates are still loading. Wait for them to finish loading, then repeat the action. Your request was not kept."
        : `Templates could not be loaded${loadError ? ` (${loadError})` : ""}. Retry loading, then repeat the action. Your request was not kept.`,
      loading ? "ok" : "error",
      { retry: !loading },
    );
    return;
  }
  // Invariant: while a write is pending the draft is locked (no edit, selection,
  // creation or discard), so what gets marked clean is exactly what was written.
  if (saving) {
    if (currentView() !== "templates") showView("templates", { force: true });
    showBanner("A save is in progress. Try again in a moment.", "ok");
    return;
  }
  if (!isDirty()) {
    if (reveal) showView("templates", { force: true });
    action();
    return;
  }
  // The guard banner lives in the templates view: make sure the user sees it.
  if (currentView() !== "templates") showView("templates", { force: true });
  askBeforeLeaving(action);
}

function askBeforeLeaving(action) {
  pendingAction = action;
  $("tpl-guard-text").textContent = `Unsaved changes in “${draft.name.trim() || "Untitled"}”. Save them before continuing?`;
  $("tpl-guard").classList.remove("hidden");
}

function closeGuard() {
  pendingAction = null;
  $("tpl-guard").classList.add("hidden");
}

async function guardSave() {
  if (saving) return;
  const action = pendingAction;
  if (await save()) {
    closeGuard();
    if (action) action();
  }
  // On failure the guard stays open, the error banner explains why and nothing is lost.
}

function guardDiscard() {
  if (saving || loading) return; // the collection is not stable to restore from
  const action = pendingAction;
  closeGuard();
  discardDraft();
  disarmDelete();
  renderAll();
  if (action) action();
}

function guardCancel() {
  if (saving) return; // the pending save will continue the action: cancelling now would contradict it
  closeGuard();
}

function discardDraft() {
  if (draft && !draft.isNew) {
    const saved = templates.find((t) => t.id === draft.id);
    draft = saved ? cloneForEditor(saved) : null;
    baseline = draft ? snapshot(draft) : null;
  } else {
    draft = null;
    baseline = null;
  }
}

// ── Banner ──────────────────────────────────────────────────

function showBanner(text, kind = "ok", { retry = false } = {}) {
  const banner = $("tpl-banner");
  $("tpl-banner-text").textContent = text;
  banner.className = `banner ${kind}`;
  $("tpl-banner-retry").classList.toggle("hidden", !retry);
}

function hideBanner() {
  $("tpl-banner").classList.add("hidden");
}

// ── Actions ─────────────────────────────────────────────────

function openTemplate(id) {
  if (!editingAllowed()) return;
  const template = templates.find((t) => t.id === id);
  if (!template) return;
  draft = cloneForEditor(template);
  baseline = snapshot(draft);
  ctx = makeContext();
  disarmDelete();
  renderAll();
}

function newTemplate(payload) {
  if (!editingAllowed()) {
    showBanner(unavailableMessage("creating a template"), loading ? "ok" : "error", { retry: !loading });
    return;
  }
  const fromMessage = typeof payload === "string";
  draft = {
    id: newTemplateId(),
    name: fromMessage ? `Message ${new Date().toLocaleTimeString()}` : "New template",
    description: "",
    payload: fromMessage ? payload : "",
    variables: [],
    isNew: true,
  };
  draft.variables = syncVariables(draft.payload, []);
  // A template built from a message is unsaved work: never considered clean.
  baseline = fromMessage ? null : snapshot(draft);
  ctx = makeContext();
  disarmDelete();
  renderAll();
  $("tpl-name").focus();
  $("tpl-name").select();
}

async function save() {
  if (!draft || saving) return false;
  if (!editingAllowed()) {
    showBanner(unavailableMessage("saving"), loading ? "ok" : "error", { retry: !loading });
    return false;
  }
  if (draft.name.trim() === "") {
    showBanner("Give the template a name before saving.", "error");
    $("tpl-name").focus();
    return false;
  }

  const saved = toPersisted(draft);
  const next = draft.isNew
    ? [...templates, saved]
    : templates.map((t) => (t.id === saved.id ? saved : t));

  saving = true;
  updateButtons();
  try {
    await invoke("save_templates", { templates: next });
  } catch (err) {
    logError(`Failed to save templates: ${String(err)}`, "templates.js:save");
    showBanner(`Could not save: ${String(err)}. Your edits are kept.`, "error");
    return false;
  } finally {
    saving = false;
    updateButtons();
  }

  // The draft is locked while saving, so it still equals what was written.
  templates = next;
  draft.isNew = false;
  draft.name = saved.name;
  baseline = snapshot(draft);
  showBanner(`Saved “${saved.name}” at ${new Date().toLocaleTimeString()}.`, "ok");
  renderAll();
  return true;
}

function disarmDelete() {
  deleteArmed = false;
  clearTimeout(deleteTimer);
  const button = $("tpl-delete");
  button.textContent = "Delete";
}

async function onDelete() {
  if (!draft || saving) return;
  if (!deleteArmed) {
    deleteArmed = true;
    $("tpl-delete").textContent = "Click again to confirm";
    deleteTimer = setTimeout(disarmDelete, 4000);
    return;
  }
  disarmDelete();

  if (draft.isNew) {
    draft = null;
    baseline = null;
    renderAll();
    return;
  }
  if (!editingAllowed()) {
    showBanner(unavailableMessage("deleting"), loading ? "ok" : "error", { retry: !loading });
    return;
  }

  const next = templates.filter((t) => t.id !== draft.id);
  saving = true;
  updateButtons();
  try {
    await invoke("save_templates", { templates: next });
  } catch (err) {
    logError(`Failed to delete template: ${String(err)}`, "templates.js:delete");
    showBanner(`Could not delete: ${String(err)}. Nothing was changed.`, "error");
    return;
  } finally {
    saving = false;
    updateButtons();
  }
  const name = draft.name;
  templates = next;
  draft = null;
  baseline = null;
  showBanner(`Deleted “${name}”.`, "ok");
  renderAll();
}

/** Resolves once for this explicit action, so preview and result are identical. */
function resolveNow() {
  ctx = makeContext();
  const result = resolveTemplate(draft.payload, manualValues(syncVariables(draft.payload, draft.variables)), ctx);
  renderPreview(result);
  return result;
}

function loadInComposer() {
  if (!draft) return;
  const result = resolveNow();
  if (!result.ok) return;
  setComposerText(result.text);
  // Leaving with unsaved edits is fine here: the draft stays in the editor.
  showView("session", { force: true });
}

async function sendNow() {
  if (!draft) return;
  const result = resolveNow();
  if (!result.ok) return;
  if (window.connection_status.get() !== "connected") {
    showBanner("Not connected: connect from Home to send.", "error");
    return;
  }
  const ok = await sendMessage(result.text);
  showBanner(ok ? "Template sent." : "Sending failed, see the conversation log.", ok ? "ok" : "error");
}

function updateSendEnabled() {
  $("tpl-send").disabled = window.connection_status.get() !== "connected";
}

// ── Editor events ───────────────────────────────────────────

/** True (and the field is restored) when an edit must be ignored. */
function editBlocked(input, value) {
  if (draft && !saving) return false;
  if (input && draft) input.value = value;
  return true;
}

function onNameInput() {
  if (editBlocked($("tpl-name"), draft?.name)) return;
  draft.name = $("tpl-name").value;
  afterEdit(false);
}

function onDescInput() {
  if (editBlocked($("tpl-desc"), draft?.description)) return;
  draft.description = $("tpl-desc").value;
  afterEdit(false);
}

function onPayloadInput() {
  if (editBlocked($("tpl-payload"), draft?.payload)) return;
  draft.payload = $("tpl-payload").value;
  draft.variables = syncVariables(draft.payload, draft.variables);
  renderPayloadDecor();
  updateCursor();
  afterEdit(true);
}

function afterEdit(variablesChanged) {
  ctx = makeContext();
  if (variablesChanged) renderVars();
  renderPreview();
  renderList();
  updateButtons();
}

function insertAtCursor(text) {
  if (!draft || saving) return;
  const area = $("tpl-payload");
  area.focus();
  area.setRangeText(text, area.selectionStart, area.selectionEnd, "end");
  onPayloadInput();
}

function insertVariable() {
  if (!draft || saving) return;
  const area = $("tpl-payload");
  const start = area.selectionStart ?? area.value.length;
  insertAtCursor("{{VAR}}");
  // Select the placeholder name so the user can type over it.
  area.setSelectionRange?.(start + 2, start + 5);
  updateCursor();
}

function updateCursor() {
  const area = $("tpl-payload");
  if (document.activeElement !== area) {
    $("tpl-cursor").textContent = "";
    return;
  }
  const before = area.value.slice(0, area.selectionStart ?? 0).split("\n");
  $("tpl-cursor").textContent = `Ln ${before.length}, Col ${before[before.length - 1].length + 1}`;
}

/** Protocol tag shown on cards and in the editor header. */
function templateKind(payload) {
  const info = describeMessage({ msg_type: "sent", content: payload });
  if (info.kind === "hl7") {
    return { label: info.hl7.version ? `HL7 ${info.hl7.version}` : "HL7", cls: "tag" };
  }
  if (info.kind === "astm") return { label: "ASTM", cls: "tag a" };
  return { label: "Text", cls: "tag c" };
}

function variableCount(payload) {
  const count = syncVariables(payload, []).length;
  return count === 1 ? "1 variable" : `${count} variables`;
}

/** Card preview: the first segment after the header (falls back to the payload start). */
function previewLine(payload) {
  const lines = stripHl7Framing(payload)
    .split(/<CR>|<LF>|\r\n|\r|\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  return (lines.find((line) => !line.startsWith("MSH")) ?? lines[0] ?? "").replace(/\s+/g, " ");
}

// ── Rendering ───────────────────────────────────────────────

function renderAll() {
  renderList();
  renderEditor();
  renderHome();
  const count = $("templates-count");
  count.textContent = String(templates.length);
  count.classList.toggle("hidden", !loaded || templates.length === 0);
  $("tpl-count-label").textContent = loaded
    ? templates.length === 1
      ? "1 template"
      : `${templates.length} templates`
    : "";
}

function renderList() {
  const list = $("tpl-list");
  const query = $("tpl-search").value.trim().toLowerCase();
  list.replaceChildren();

  const matches = (t) =>
    query === "" ||
    `${t.name}\n${t.description ?? ""}\n${t.payload}`.toLowerCase().includes(query);

  const rows = templates.filter(matches);
  if (draft?.isNew) rows.unshift(draft);

  if (rows.length === 0) {
    list.appendChild(
      el("p", "empty-note", loading ? "Loading templates…" : !loaded ? "Templates unavailable." : query ? "No match." : "No template yet."),
    );
    return;
  }

  for (const row of rows) {
    const active = draft && draft.id === row.id;
    // The editor holds the live copy of the selected template.
    const shown = active ? draft : row;
    const card = el("button", `tcard${active ? " on" : ""}`);
    card.type = "button";
    const title = el("b");
    title.appendChild(el("span", "tname", shown.name.trim() || "Untitled"));
    if (active && isDirty()) title.appendChild(el("span", "dot", "●"));
    card.appendChild(title);
    const preview = el("div", "pv");
    const line = previewLine(shown.payload);
    if (line) renderTokens(preview, line, { segments: true });
    else preview.textContent = "(empty)";
    card.appendChild(preview);
    const meta = el("div", "mt");
    const kind = templateKind(shown.payload);
    meta.appendChild(el("span", kind.cls, kind.label));
    meta.appendChild(el("span", "", variableCount(shown.payload)));
    if (active && draft.isNew) meta.appendChild(el("span", "tag s", "unsaved"));
    card.appendChild(meta);
    card.addEventListener("click", () => {
      if (active) return;
      requestAction(() => openTemplate(row.id));
    });
    list.appendChild(card);
  }
}

function renderEditor() {
  const has = draft !== null;
  $("tpl-empty").classList.toggle("hidden", has);
  $("tpl-form").classList.toggle("hidden", !has);
  $("tpl-new").disabled = !editingAllowed();
  $("tpl-new-card").disabled = !editingAllowed();
  if (!has) return;

  if ($("tpl-name").value !== draft.name) $("tpl-name").value = draft.name;
  if ($("tpl-desc").value !== draft.description) $("tpl-desc").value = draft.description;
  if ($("tpl-payload").value !== draft.payload) $("tpl-payload").value = draft.payload;
  renderPayloadDecor();
  updateCursor();
  renderVars();
  renderPreview();
  updateButtons();
}

/** Header tag + highlight mirror behind the payload textarea, and textarea auto-grow. */
function renderPayloadDecor() {
  if (!draft) return;
  const kind = templateKind(draft.payload);
  const tag = $("tpl-kind");
  tag.className = kind.cls;
  tag.textContent = kind.label;
  $("tpl-var-count").textContent = variableCount(draft.payload);

  const mirror = $("tpl-payload-hl");
  mirror.replaceChildren();
  for (const part of splitTokens(draft.payload)) {
    if (part.type === "ctrl") {
      mirror.appendChild(el("span", "mc", `<${part.value}>`));
      continue;
    }
    for (const piece of part.value.split(/(\{\{[^}]*\}\})/)) {
      if (piece === "") continue;
      if (/^\{\{[^}]*\}\}$/.test(piece)) mirror.appendChild(el("span", "mv", piece));
      else mirror.appendChild(document.createTextNode(piece));
    }
  }
  // Keeps the height of a trailing empty line in sync with the textarea.
  mirror.appendChild(document.createTextNode("\u200b"));

  const area = $("tpl-payload");
  const nativeSizing = globalThis.CSS?.supports?.("field-sizing", "content");
  if (area.style && !nativeSizing) {
    area.style.height = "auto";
    area.style.height = `${area.scrollHeight + 2}px`;
  }
}

function updateButtons() {
  $("tpl-guard-cancel").disabled = saving;
  $("tpl-guard-discard").disabled = saving;
  $("tpl-guard-save").disabled = saving;
  if (!draft) return;
  const allowed = editingAllowed() && !saving;
  $("tpl-save").disabled = !allowed;
  $("tpl-delete").disabled = !allowed;
  for (const id of ["tpl-name", "tpl-desc", "tpl-payload"]) $(id).readOnly = saving;
  for (const input of document.querySelectorAll("#tpl-vars input")) input.readOnly = saving;
  $("tpl-dirty").classList.toggle("hidden", !isDirty());
  $("tpl-saved").classList.toggle("hidden", isDirty() || Boolean(draft.isNew));
}

function renderVars() {
  const container = $("tpl-vars");
  container.replaceChildren();
  const names = extractVariables(draft.payload);
  $("tpl-vars-label").textContent = names.length ? `Variables (${names.length})` : "Variables";

  if (names.length === 0) {
    container.appendChild(el("p", "empty-note", "No variable in this payload."));
    return;
  }
  for (const name of names) {
    const row = el("div", "vr");
    const label = el("label", "lbl");
    label.appendChild(el("span", "var", name));
    if (isAutoVariable(name)) {
      label.appendChild(el("em", "", "auto"));
      row.appendChild(label);
      row.appendChild(
        el("div", "inp auto", name === "NOW" ? "yyyyMMddHHmmss" : "unique per send"),
      );
    } else {
      label.appendChild(el("em", "", "value"));
      row.appendChild(label);
      const variable = draft.variables.find((v) => v.name === name);
      const input = el("input");
      input.type = "text";
      input.value = variable ? variable.default : "";
      input.placeholder = "required";
      input.setAttribute("aria-label", `Value of ${name}`);
      input.readOnly = saving;
      input.addEventListener("input", () => {
        if (!draft || saving) {
          const current = draft?.variables.find((v) => v.name === name);
          input.value = current ? current.default : "";
          return;
        }
        const target = draft.variables.find((v) => v.name === name);
        if (target) target.default = input.value;
        afterEdit(false);
      });
      row.appendChild(input);
    }
    container.appendChild(row);
  }
}

function renderPreviewText(container, text) {
  container.replaceChildren();
  for (const part of splitTokens(text)) {
    if (part.type === "ctrl") {
      const chip = el("span", "cc", part.value);
      chip.title = `<${part.value}>`;
      container.appendChild(chip);
      continue;
    }
    // Highlight unresolved placeholders.
    for (const piece of part.value.split(/(\{\{[^}]*\}\})/)) {
      if (piece === "") continue;
      if (/^\{\{[^}]*\}\}$/.test(piece)) container.appendChild(el("span", "var", piece));
      else container.appendChild(document.createTextNode(piece));
    }
  }
}

function renderPreview(result) {
  if (!draft) return;
  const resolved =
    result ??
    resolveTemplate(draft.payload, manualValues(syncVariables(draft.payload, draft.variables)), ctx);
  renderPreviewText($("tpl-preview"), resolved.text);

  const validity = $("tpl-validity");
  validity.replaceChildren();
  if (draft.payload === "") {
    validity.className = "validity warn";
    validity.textContent = "Empty payload: nothing to send.";
  } else if (resolved.ok) {
    validity.className = "validity ok";
    const parts = ["✓ Ready"];
    const hl7 = parseHl7(resolved.text);
    if (hl7) {
      parts.push(hl7.version ? `HL7 ${hl7.version}` : "HL7");
      const count = hl7.segments.length;
      parts.push(count === 1 ? "1 segment" : `${count} segments`);
      if (hl7.messageType) parts.push(`MSH-9 = ${hl7.messageType}`);
    }
    parts.push(`${resolved.text.length} chars`);
    validity.textContent = parts.join(" · ");
    if (/[\r\n]/.test(resolved.text)) {
      validity.className = "validity warn";
      validity.textContent += " · real line breaks are sent as separate frames";
    }
  } else {
    validity.className = "validity err";
    validity.textContent = resolved.errors.join(" ");
  }
}

function renderHome() {
  const container = $("home-templates");
  container.replaceChildren();
  $("home-templates-empty").classList.toggle("hidden", templates.length > 0);
  $("home-templates-empty").textContent = loading
    ? "Loading templates…"
    : !loaded
      ? `Templates unavailable${loadError ? `: ${loadError}` : ""}`
      : "No template yet.";

  for (const template of templates.slice(0, 6)) {
    const card = el("button", "rc");
    card.type = "button";
    const top = el("div", "top");
    const kind = templateKind(template.payload);
    top.appendChild(el("span", kind.cls, kind.label));
    top.appendChild(el("span", "go", "Open ↗"));
    card.appendChild(top);
    card.appendChild(el("div", "ep plain", template.name));
    card.appendChild(el("div", "ds", template.description || variableCount(template.payload)));
    card.addEventListener("click", () =>
      requestAction(() => {
        showView("templates", { force: true });
        openTemplate(template.id);
      }),
    );
    container.appendChild(card);
  }
}
