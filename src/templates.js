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
import { sendMessage, setComposerText, activeSendTarget, activeSessionId } from "./messages.js";
import { SAVE_AS_TEMPLATE_EVENT } from "./inspector.js";
import { subscribeSendEligibility } from "./sessions.js";
import { el, renderTokens } from "./render.js";
import { logError, logInfo } from "./log.js";
import { publishSavedTemplates, clearSavedTemplates } from "./template-store.js";
import { applyTemplateAutobuild } from "./autobuild-core.js";

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
/** True while a template is being built by Autobuild before send/load (blocks double actions). */
let delivering = false;
/** Template id whose built text the preview box currently shows (null: plain text or error). */
let builtShownFor = null;
/** Monotonic id of the latest built-preview request (older results are ignored). */
let previewSeq = 0;

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
  $("tpl-payload").addEventListener("input", onPayloadInput);
  $("tpl-save").addEventListener("click", () => void save());
  $("tpl-delete").addEventListener("click", () => void onDelete());
  $("tpl-load").addEventListener("click", () => void loadInComposer());
  $("tpl-send").addEventListener("click", () => void sendNow());
  $("tpl-autobuild").addEventListener("change", onAutobuildChange);
  $("tpl-autobuild-noetb").addEventListener("change", onAutobuildChange);
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

  subscribeSendEligibility(updateSendEnabled);

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
    publishSavedTemplates(templates);
    if (result.seeded) logInfo("Built-in templates provided (no saved templates yet)", "templates.js:load");
  } catch (err) {
    templates = [];
    loadError = String(err);
    loaded = false;
    clearSavedTemplates();
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
    autobuild: Boolean(template.autobuild),
    autobuildNoEtb: Boolean(template.autobuild_no_etb),
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
    autobuild: Boolean(d.autobuild),
    autobuild_no_etb: Boolean(d.autobuildNoEtb),
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
    autobuild: false,
    autobuildNoEtb: false,
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
  publishSavedTemplates(templates);
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
  publishSavedTemplates(templates);
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

async function loadInComposer() {
  if (!draft || delivering) return;
  const result = resolveNow();
  if (!result.ok) return;
  // Captured now, before any await: the text goes to the session selected at this moment.
  const target = activeSessionId();
  if (!target) {
    showBanner("No session is open: start one from Home, then load the template.", "error");
    return;
  }
  const options = autobuildOptions();
  // Also captured now: the view and the draft the user initiated this from.
  const startedInTemplates = currentView() === "templates";
  const startDraft = draft;
  const text = await buildForDelivery(result.text, options, "loaded");
  if (text === null) return;
  if (!setComposerText(text, target)) {
    showBanner(
      options.autobuild
        ? "The session this template was built for has been closed: nothing was loaded."
        : "No session is open: start one from Home, then load the template.",
      "error",
    );
    return;
  }
  // Navigate only if the user is still where they started; otherwise the late result
  // must not hijack the navigation nor bypass another view's leave guard.
  if (!startedInTemplates || currentView() !== "templates" || draft !== startDraft) return;
  // Leaving with unsaved edits is fine here: the draft stays in the editor.
  showView("session", { force: true });
}

async function sendNow() {
  if (!draft || delivering) return;
  const result = resolveNow();
  if (!result.ok) return;
  // The target is captured before awaiting: a session switch cannot redirect the send.
  const target = activeSendTarget();
  if (!target) {
    showBanner("The selected session is not connected: connect it to send.", "error");
    return;
  }
  const text = await buildForDelivery(result.text, autobuildOptions(), "sent");
  if (text === null) return;
  const ok = await sendMessage(text, target);
  showBanner(
    ok ? `Template sent to ${target.label}.` : `Sending to ${target.label} failed, see its conversation log.`,
    ok ? "ok" : "error",
  );
}

/** Autobuild options of the draft as they are right now (captured before any await). */
function autobuildOptions() {
  return { autobuild: Boolean(draft.autobuild), noEtb: Boolean(draft.autobuildNoEtb) };
}

/**
 * Applies the template's Autobuild option to resolved text. Returns the text to deliver, or
 * null (after showing the error) when the build failed: nothing may be delivered then.
 * Re-entry is blocked while the build is pending.
 */
async function buildForDelivery(text, options, verb) {
  if (!options.autobuild) return text;
  delivering = true;
  updateSendEnabled();
  try {
    return await applyTemplateAutobuild({ invoke }, text, options);
  } catch (err) {
    logError(`Autobuild failed for template: ${String(err)}`, "templates.js:autobuild");
    showBanner(`Autobuild failed: ${String(err)}. Nothing was ${verb}.`, "error");
    return null;
  } finally {
    delivering = false;
    updateSendEnabled();
  }
}

function updateSendEnabled() {
  const target = activeSendTarget();
  $("tpl-send").disabled = !target || delivering;
  $("tpl-load").disabled = delivering;
  $("tpl-send").textContent = target ? `Send to ${target.label} ➤` : "Send now ➤";
  $("tpl-send").title = target
    ? `Send to the selected session (${target.label})`
    : "Select a connected session to send";
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

function onAutobuildChange() {
  if (!draft || saving) {
    syncAutobuildControls(); // restore the controls: the edit is refused
    return;
  }
  draft.autobuild = $("tpl-autobuild").checked;
  draft.autobuildNoEtb = $("tpl-autobuild-noetb").checked;
  syncAutobuildControls();
  afterEdit(false);
}

/** Mirrors the draft in the Autobuild switches ("No ETB" only applies when Autobuild is on). */
function syncAutobuildControls() {
  const box = $("tpl-autobuild");
  const sub = $("tpl-autobuild-noetb");
  if (draft) {
    box.checked = Boolean(draft.autobuild);
    sub.checked = Boolean(draft.autobuildNoEtb);
  }
  box.disabled = saving;
  sub.disabled = saving || !draft?.autobuild;
}

function onPayloadInput() {
  if (editBlocked($("tpl-payload"), draft?.payload)) return;
  draft.payload = $("tpl-payload").value;
  draft.variables = syncVariables(draft.payload, draft.variables);
  renderPayloadDecor();
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
}

/** Protocol tag shown on cards and in the editor header. */
function templateKind(payload) {
  const info = describeMessage({ msg_type: "sent", content: payload });
  if (info.kind === "hl7") {
    return { label: info.hl7.version ? `HL7 ${info.hl7.version}` : "HL7", cls: "badge" };
  }
  if (info.kind === "astm") return { label: "ASTM", cls: "badge in" };
  return { label: "Text", cls: "badge mute" };
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
}

function renderList() {
  const list = $("tpl-list");
  const query = $("tpl-search").value.trim().toLowerCase();
  list.replaceChildren();

  const matches = (t) =>
    query === "" ||
    `${t.name}\n${t.payload}`.toLowerCase().includes(query);

  const rows = templates.filter(matches);
  if (draft?.isNew) rows.unshift(draft);

  if (rows.length === 0) {
    list.appendChild(
      el("p", "", loading ? "Loading templates…" : !loaded ? "Templates unavailable." : query ? "No match." : "No template yet."),
    );
    return;
  }

  for (const row of rows) {
    const active = draft && draft.id === row.id;
    // The editor holds the live copy of the selected template.
    const shown = active ? draft : row;
    const card = el("button", `card${active ? " on" : ""}`);
    card.type = "button";
    const title = el("b");
    title.appendChild(el("span", "", shown.name.trim() || "Untitled"));
    if (active && isDirty()) title.appendChild(el("span", "", "●"));
    card.appendChild(title);
    const preview = el("div", "pv");
    const line = previewLine(shown.payload);
    if (line) renderTokens(preview, line, { segments: true });
    else preview.textContent = "(empty)";
    card.appendChild(preview);
    const meta = el("div", "mt");
    const kind = templateKind(shown.payload);
    meta.appendChild(el("span", kind.cls, kind.label));
    if (shown.autobuild) meta.appendChild(el("span", "badge outline", "Autobuild"));
    if (active && draft.isNew) meta.appendChild(el("span", "badge warn", "unsaved"));
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
  if ($("tpl-payload").value !== draft.payload) $("tpl-payload").value = draft.payload;
  renderPayloadDecor();
  syncAutobuildControls();
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
  for (const id of ["tpl-name", "tpl-payload"]) $(id).readOnly = saving;
  for (const input of document.querySelectorAll("#tpl-vars input")) input.readOnly = saving;
  syncAutobuildControls();
  $("tpl-dirty").classList.toggle("hidden", !isDirty());
  $("tpl-saved").classList.toggle("hidden", isDirty() || Boolean(draft.isNew));
}

function renderVars() {
  const container = $("tpl-vars");
  container.replaceChildren();
  const names = extractVariables(draft.payload);

  if (names.length === 0) return;
  for (const name of names) {
    const row = el("div");
    const label = el("label");
    label.appendChild(el("span", "var", name));
    if (isAutoVariable(name)) {
      label.appendChild(el("em", "", "auto"));
      row.appendChild(label);
      row.appendChild(
        el("output", "", name === "NOW" ? "yyyyMMddHHmmss" : "unique per send"),
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
  const validity = $("tpl-validity");
  validity.replaceChildren();
  if (draft.payload === "") {
    validity.className = "warn";
    validity.textContent = "Empty payload: nothing to send.";
  } else if (resolved.ok) {
    validity.className = "ok";
    const parts = ["✓ Ready"];
    const hl7 = parseHl7(resolved.text);
    if (hl7) {
      parts.push(hl7.version ? `HL7 ${hl7.version}` : "HL7");
      if (hl7.messageType) parts.push(`MSH-9 = ${hl7.messageType}`);
    }
    validity.textContent = parts.join(" · ");
  } else {
    validity.className = "error";
    validity.textContent = resolved.errors.join(" ");
  }
  renderPreviewBox(resolved);
}

/**
 * Single preview box. Autobuild off (or nothing buildable): the resolved text, synchronously.
 * Autobuild on: the built text, fetched asynchronously. Every call supersedes the previous
 * one: a result that arrives late (older request, other template, switch turned off, editor
 * closed) is dropped. While a build is pending the box keeps the last built text of the same
 * template (dimmed) or, failing that, the plain resolved text (dimmed), so typing never
 * flickers; a failed build shows the error instead of any text.
 */
function renderPreviewBox(resolved) {
  const seq = ++previewSeq;
  const out = $("tpl-preview");
  const tag = $("tpl-preview-tag");
  const building = draft.autobuild && draft.payload !== "" && resolved.ok;
  tag.classList.toggle("hidden", !draft.autobuild);
  if (!building) {
    builtShownFor = null;
    out.className = "raw";
    out.textContent = "";
    renderPreviewText(out, resolved.text);
    return;
  }
  const id = draft.id;
  if (builtShownFor !== id) {
    builtShownFor = null;
    out.textContent = "";
    renderPreviewText(out, resolved.text);
  }
  out.className = "raw pending";
  const current = () => seq === previewSeq && draft !== null && draft.id === id;
  applyTemplateAutobuild({ invoke }, resolved.text, autobuildOptions()).then(
    (text) => {
      if (!current()) return;
      out.className = "raw";
      out.textContent = "";
      renderPreviewText(out, text);
      builtShownFor = id;
    },
    (err) => {
      if (!current()) return;
      builtShownFor = null;
      out.className = "raw error";
      out.replaceChildren();
      out.textContent = `Autobuild failed: ${String(err)}`;
    },
  );
}