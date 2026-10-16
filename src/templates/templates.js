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
import { tokenPayload, splitTokens } from "../control-chars.js";
import { describeMessage, parseHl7 } from "../session/inspector-core.js";
import { showView, setLeaveGuard, currentView } from "../nav.js";
import { sendMessage, setComposerText, activeSendTarget, activeSessionId } from "../session/messages.js";
import { SAVE_AS_TEMPLATE_EVENT } from "../session/inspector.js";
import { subscribeSendEligibility } from "../session/sessions.js";
import { el } from "../render.js";
import { cloneTemplate } from "../include.js";
import { logError, logInfo } from "../log.js";
import { publishSavedTemplates, clearSavedTemplates } from "./template-store.js";
import { applyTemplateAutobuild } from "../autobuild/autobuild-core.js";

const TEMPLATE_CARD = "templates/template-card.html";
const VARIABLE_AUTO_ROW = "templates/variable-auto-row.html";
const VARIABLE_VALUE_ROW = "templates/variable-value-row.html";

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
/** Latest variable values per saved template id (persisted in the background, never via the draft). */
const liveVariables = new Map();
let varSavePromise = null;
let varSaveQueued = false;
let pendingAction = null;
let deleteArmed = false;
let deleteTimer = null;
/** The ✕ rendered on the selected card, if any. */
let deleteButton = null;
/** True while a template is being built by Autobuild before send/load (blocks double actions). */
let delivering = false;
/** Template id whose built text the preview box currently shows (null: plain text or error). */
let builtShownFor = null;
/** Monotonic id of the latest built-preview request (older results are ignored). */
let previewSeq = 0;

const $ = (id) => document.getElementById(id);

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

  window.actions ??= {};
  window.actions.tpl = {
    newTemplate: () => requestAction(newTemplate),
    search: () => renderList(),
    nameInput: () => onNameInput(),
    payloadInput: () => onPayloadInput(),
    save: () => void save(),
    loadInComposer: () => void loadInComposer(),
    send: () => void sendNow(),
    autobuild: () => onAutobuildChange(),
    insertVariable: () => insertVariable(),
    insertToken: (_ev, button) => {
      const text = tokenPayload(button.dataset.insert);
      if (text !== null) insertAtCursor(text);
    },
    bannerClose: () => hideBanner(),
    bannerRetry: () => void loadAll(),
    guardSave: () => void guardSave(),
    guardDiscard: () => guardDiscard(),
    guardCancel: () => guardCancel(),
    open: (_ev, button) => {
      const id = button.closest("[data-id]")?.dataset.id;
      if (draft && String(draft.id) === id) return;
      requestAction(() => openTemplate(templates.find((t) => String(t.id) === id)?.id ?? id));
    },
    remove: () => void onDelete(),
    variable: (_ev, input) => onVariableInput(input),
  };

  document.addEventListener(SAVE_AS_TEMPLATE_EVENT, (ev) =>
    requestAction(() => newTemplate(ev.detail.payload), { reveal: true }),
  );

  window.addEventListener("beforeunload", (ev) => {
    if (isDirty() || saving || varSavePromise !== null || varSaveQueued) {
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
  if (saving || varSavePromise) return Promise.resolve(loaded); // never replace the list under a pending write
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
    liveVariables.clear();
    loadError = null;
    loaded = true;
    ok = true;
    publishSavedTemplates(templates);
    if (result.seeded) logInfo("Built-in templates provided (no saved templates yet)", "templates.js:load");
  } catch (err) {
    templates = [];
    liveVariables.clear();
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

/** Overlay `overrides` on a copy of `base` (update in place or append). */
function mergeVariables(base = [], overrides = []) {
  const merged = base.map((v) => ({ name: v.name, default: v.default }));
  for (const { name, default: val } of overrides) {
    const existing = merged.find((v) => v.name === name);
    if (existing) existing.default = val;
    else merged.push({ name, default: val });
  }
  return merged;
}

function cloneForEditor(template) {
  return {
    id: template.id,
    name: template.name,
    description: template.description ?? "",
    payload: template.payload,
    autobuild: Boolean(template.autobuild),
    autobuildNoEtb: Boolean(template.autobuild_no_etb),
    variables: syncVariables(template.payload, mergeVariables(template.variables, liveVariables.get(template.id))),
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

/** Saved templates with the latest live variable values applied (nothing else changes). */
function withLiveVariables(list) {
  return list.map((t) => ({
    ...t,
    variables: syncVariables(t.payload, mergeVariables(t.variables, liveVariables.get(t.id))).map((v) => ({
      name: v.name,
      default: v.default,
    })),
  }));
}

function scheduleVariableSave() {
  if (varSavePromise) {
    varSaveQueued = true;
    return;
  }
  varSavePromise = persistVariables().finally(() => {
    varSavePromise = null;
  });
}

/** Serialized background write of variable values only; coalesces edits made meanwhile. */
async function persistVariables() {
  do {
    varSaveQueued = false;
    if (saving || !loaded) return;
    const next = withLiveVariables(templates);
    const same = JSON.stringify(next.map((t) => t.variables)) ===
      JSON.stringify(templates.map((t) => (t.variables ?? []).map((v) => ({ name: v.name, default: v.default }))));
    if (same) continue;
    try {
      await invoke("save_templates", { templates: next });
    } catch (err) {
      logError(`Failed to save variable values: ${String(err)}`, "templates.js:variables");
      if (!saving) showBanner(`Could not save: ${String(err)}.`, "error");
      if (varSaveQueued && !saving && editingAllowed()) continue;
      return;
    }
    if (loaded && !loading) {
      templates = next;
      publishSavedTemplates(templates);
      hideBanner();
    }
    if (saving || !loaded) return;
  } while (varSaveQueued);
}

function savedVariablesFor(id) {
  return mergeVariables(templates.find((t) => t.id === id)?.variables ?? [], liveVariables.get(id) ?? []);
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
  const resumeVarSave = varSaveQueued || varSavePromise !== null;
  varSaveQueued = false;
  saving = true;
  updateButtons();
  if (varSavePromise !== null) await varSavePromise.catch(() => {});
  const base = withLiveVariables(templates);
  const next = draft.isNew
    ? [...base, saved]
    : base.map((t) => (t.id === saved.id ? saved : t));
  try {
    await invoke("save_templates", { templates: next });
  } catch (err) {
    logError(`Failed to save templates: ${String(err)}`, "templates.js:save");
    showBanner(`Could not save: ${String(err)}. Your edits are kept.`, "error");
    saving = false;
    if (resumeVarSave) scheduleVariableSave();
    return false;
  } finally {
    saving = false;
    updateButtons();
  }

  // The draft is locked while saving, so it still equals what was written.
  templates = next;
  liveVariables.set(saved.id, saved.variables.map((v) => ({ ...v })));
  publishSavedTemplates(templates);
  draft.isNew = false;
  draft.name = saved.name;
  baseline = snapshot(draft);
  hideBanner();
  renderAll();
  return true;
}

function disarmDelete() {
  deleteArmed = false;
  clearTimeout(deleteTimer);
  syncDeleteButton();
}

/** The ✕ of the selected card: asks for a second click, which deletes the template. */
function syncDeleteButton() {
  if (!deleteButton) return;
  const name = draft ? draft.name.trim() || "Untitled" : "";
  deleteButton.textContent = deleteArmed ? "Sure?" : "✕";
  deleteButton.classList.toggle("armed", deleteArmed);
  const label = deleteArmed ? `Click again to delete template ${name}` : `Delete template ${name}`;
  deleteButton.title = deleteArmed ? "Click again to delete" : "Delete template";
  deleteButton.setAttribute("aria-label", label);
}

async function onDelete() {
  if (!draft || saving) return;
  if (!deleteArmed) {
    deleteArmed = true;
    syncDeleteButton();
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

  const resumeVarSave = varSaveQueued || varSavePromise !== null;
  varSaveQueued = false;
  saving = true;
  updateButtons();
  if (varSavePromise !== null) await varSavePromise.catch(() => {});
  const next = withLiveVariables(templates).filter((t) => t.id !== draft.id);
  try {
    await invoke("save_templates", { templates: next });
  } catch (err) {
    logError(`Failed to delete template: ${String(err)}`, "templates.js:delete");
    showBanner(`Could not delete: ${String(err)}. Nothing was changed.`, "error");
        saving = false;
        if (resumeVarSave) scheduleVariableSave();
        return;
  } finally {
    saving = false;
    updateButtons();
  }
  const deletedId = draft.id;
  templates = next;
  liveVariables.delete(deletedId);
  publishSavedTemplates(templates);
  draft = null;
  baseline = null;
  hideBanner();
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
  if (ok) hideBanner();
  else showBanner(`Sending to ${target.label} failed, see its conversation log.`, "error");
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
  draft.variables = syncVariables(
    draft.payload,
    mergeVariables(draft.isNew ? [] : savedVariablesFor(draft.id), draft.variables),
  );
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
    return { label: "HL7", cls: "badge" };
  }
  if (info.kind === "astm") return { label: "ASTM", cls: "badge in" };
  return { label: "Text", cls: "badge mute" };
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
  deleteButton = null;

  // The open template stays listed so its ✕ is always reachable.
  const matches = (t) =>
    query === "" ||
    draft?.id === t.id ||
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
    const name = shown.name.trim() || "Untitled";
    const item = cloneTemplate(TEMPLATE_CARD, { name });
    const part = (slot) => item.querySelector(`[data-slot="${slot}"]`);
    item.classList.toggle("on", Boolean(active));
    const nameText = part("name");
    nameText.title = name;
    const kind = templateKind(shown.payload);
    const kindTag = part("kind");
    kindTag.className = kind.cls;
    kindTag.textContent = kind.label;
    if (!(active && isDirty())) part("dirty").remove();
    item.dataset.id = row.id;
    const del = part("delete");
    if (active) {
      del.disabled = !editingAllowed() || saving;
      deleteButton = del;
      syncDeleteButton();
    } else {
      del.remove();
    }
    list.appendChild(item);
  }
}

function renderEditor() {
  const has = draft !== null;
  $("tpl-empty").classList.toggle("hidden", has);
  $("tpl-form").classList.toggle("hidden", !has);
  $("tpl-new").disabled = !editingAllowed();
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
  if (deleteButton) deleteButton.disabled = !allowed;
  for (const id of ["tpl-name", "tpl-payload"]) $(id).readOnly = saving;
  for (const input of document.querySelectorAll("#tpl-vars input")) input.readOnly = saving;
  syncAutobuildControls();
  $("tpl-dirty").classList.toggle("hidden", !isDirty());
  $("tpl-form").classList.toggle("pending", isDirty());
}

function renderVars() {
  const container = $("tpl-vars");
  container.replaceChildren();
  const names = extractVariables(draft.payload);

  if (names.length === 0) return;
  for (const name of names) {
    if (isAutoVariable(name)) {
      container.appendChild(
        cloneTemplate(VARIABLE_AUTO_ROW, {
          name,
          sample: name === "NOW" ? "yyyyMMddHHmmss" : "unique per send",
        }),
      );
      continue;
    }
    const row = cloneTemplate(VARIABLE_VALUE_ROW, { name });
    const variable = draft.variables.find((v) => v.name === name);
    const input = row.querySelector('[data-slot="input"]');
    input.value = variable ? variable.default : "";
    input.setAttribute("aria-label", `Value of ${name}`);
    input.readOnly = saving;
    input.dataset.name = name;
    container.appendChild(row);
  }
}

function onVariableInput(input) {
  const name = input.dataset.name;
  if (!draft || saving) {
    const current = draft?.variables.find((v) => v.name === name);
    input.value = current ? current.default : "";
    return;
  }
  const target = draft.variables.find((v) => v.name === name);
  if (target) target.default = input.value;
  if (!draft.isNew) {
    liveVariables.set(
      draft.id,
      mergeVariables(savedVariablesFor(draft.id), [{ name, default: input.value }]),
    );
    scheduleVariableSave();
  }
  afterEdit(false);
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
    if (parseHl7(resolved.text)) parts.push("HL7");
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
  const building = draft.autobuild && draft.payload !== "" && resolved.ok;
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

await init();
