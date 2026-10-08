// Auto reply view: ordered, global rules (first enabled match wins) with a docked rule editor.
// Everything is configured in the rules; there is no default acknowledgement.
//
// Persistence goes through the backend config file (load_auto_reply / save_auto_reply /
// set_auto_reply_enabled). The master switch persists ONLY the enabled flag, never the draft.
// Template choices come from the saved template library (template-store.js), never from drafts.
import {
  describeAction,
  describeCondition,
  describeDelay,
  describeTrigger,
  moveRule,
  newRule,
  previewTemplate,
  readLegacySettings,
  removeLegacySettings,
  ruleFromPersisted,
  ruleToPersisted,
  snapshotRules,
  validateRule,
  validateRules,
  variableSources,
} from "./autoreply-core.js";
import { getSavedTemplates, subscribeSavedTemplates } from "./template-store.js";
import { setLeaveGuard } from "./nav.js";
import { el, renderTokens } from "./render.js";
import { logError, logInfo } from "./log.js";

const { invoke } = window.__TAURI__.core;

/** Draft rules (editing model, in priority order). */
let rules = [];
/** Snapshot of the persisted rules; the draft is dirty when it differs (or when imported). */
let baseline = snapshotRules([]);
let imported = false;
/** Persisted master switch (what the backend actually applies). */
let enabled = false;
let loaded = false;
let loading = false;
let loadError = null;
let loadPromise = null;
let saving = false;
let togglingMaster = false;
let selectedId = null;
let deleteArmedId = null;
let deleteTimer = null;
let templates = getSavedTemplates();

const $ = (id) => document.getElementById(id);

window.addEventListener("DOMContentLoaded", init);

async function init() {
  setLeaveGuard("autoreply", (proceed) => {
    if (saving) {
      showBanner("A save is in progress. Try again in a moment.", "ok");
      return false;
    }
    if (!isDirty()) return true;
    askBeforeLeaving(proceed);
    return false;
  });

  $("ar-add").addEventListener("click", addRule);
  $("ar-save").addEventListener("click", () => void save());
  $("ar-discard").addEventListener("click", discard);
  $("ar-enabled").addEventListener("change", () => void toggleMaster());
  $("ar-banner-close").addEventListener("click", hideBanner);
  $("ar-banner-retry").addEventListener("click", () => void loadAll());
  $("ar-guard-save").addEventListener("click", () => void guardSave());
  $("ar-guard-discard").addEventListener("click", guardDiscard);
  $("ar-guard-cancel").addEventListener("click", guardCancel);
  $("ar-form").addEventListener("submit", (ev) => ev.preventDefault());

  const text = (id, field) => $(id).addEventListener("input", () => edit((r) => (r[field] = $(id).value)));
  const change = (id, field, read = () => $(id).value) =>
    $(id).addEventListener("change", () => edit((r) => (r[field] = read()), true));
  text("ar-name", "name");
  text("ar-type", "messageType");
  text("ar-cond-seg", "condSegment");
  text("ar-cond-field", "condField");
  text("ar-cond-value", "condValue");
  text("ar-literal", "literal");
  text("ar-ack-type", "ackType");
  text("ar-ack-code", "ackCode");
  text("ar-delay", "delay");
  change("ar-rule-enabled", "enabled", () => $("ar-rule-enabled").checked);
  change("ar-trigger", "trigger");
  change("ar-cond-use", "useCondition", () => $("ar-cond-use").checked);
  change("ar-cond-op", "condOperator");
  change("ar-action", "action");
  change("ar-template", "templateId");

  window.addEventListener("beforeunload", (ev) => {
    if (isDirty() || saving) {
      ev.preventDefault();
      ev.returnValue = "";
    }
  });

  templates = getSavedTemplates();
  subscribeSavedTemplates((list) => {
    templates = list;
    renderAll();
  });

  await loadAll();
}

// ── State helpers ───────────────────────────────────────────

function isDirty() {
  return loaded && (imported || snapshotRules(rules) !== baseline);
}

function selectedRule() {
  return rules.find((r) => r.id === selectedId) ?? null;
}

function editable() {
  return loaded && !loading && !saving;
}

function unavailable(what) {
  return loading
    ? `The rules are still loading: ${what} is unavailable for a moment.`
    : `${what[0].toUpperCase()}${what.slice(1)} is disabled: the rules could not be loaded${loadError ? ` (${loadError})` : ""}.`;
}

// ── Loading ─────────────────────────────────────────────────

/** In-flight loads are shared; a failed load blocks every write (nothing is overwritten blind). */
function loadAll() {
  if (saving || togglingMaster) return Promise.resolve(loaded);
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
  let banner = null;
  try {
    const result = await invoke("load_auto_reply");
    enabled = Boolean(result.config.enabled);
    ok = true;
    if (isDirty() && loaded) {
      // A reload never replaces edits in progress.
    } else {
      rules = result.config.rules.map(ruleFromPersisted);
      baseline = snapshotRules(rules);
      imported = false;
      if (!result.present) banner = importLegacy();
    }
    loadError = null;
    loaded = true;
  } catch (err) {
    loadError = String(err);
    loaded = false;
    enabled = false;
    rules = [];
    baseline = snapshotRules([]);
    imported = false;
    selectedId = null;
    logError(`Failed to load auto reply rules: ${loadError}`, "autoreply.js:load");
  } finally {
    loading = false;
  }
  if (!ok) {
    showBanner(
      `Could not load the auto reply rules: ${loadError}. Automatic replies are off and saving is disabled so the file is not overwritten.`,
      "error",
      { retry: true },
    );
  } else if (banner) {
    showBanner(banner.text, banner.kind);
  } else {
    hideBanner();
  }
  if (selectedId && !selectedRule()) selectedId = null;
  renderAll();
  return ok;
}

/** Previous (localStorage) settings become an explicit, unsaved draft. Returns a banner or null. */
function importLegacy() {
  const legacy = readLegacySettings(window.localStorage ?? globalThis.localStorage);
  if (legacy === null) return null;
  if (legacy.error) {
    logError(legacy.error, "autoreply.js:legacy");
    return {
      kind: "error",
      text: `${legacy.error} They were left untouched and nothing was imported.`,
    };
  }
  if (legacy.rules.length === 0) return null;
  rules = legacy.rules;
  imported = true;
  selectedId = rules[0].id;
  logInfo(`Imported previous auto reply settings as ${rules.length} draft rule(s)`, "autoreply.js:legacy");
  return {
    kind: "warn",
    text: `Imported previous settings — review and Save. Automatic replies stay off until you save the rules and turn Enabled on.`,
  };
}

// ── Editing ─────────────────────────────────────────────────

function newId() {
  return newRule().id;
}

function addRule() {
  if (!editable()) {
    showBanner(unavailable("adding a rule"), loading ? "ok" : "error", { retry: !loading && !saving });
    return;
  }
  const rule = newRule(newId());
  rules = [...rules, rule];
  selectedId = rule.id;
  renderAll();
  $("ar-name").focus();
  $("ar-name").select();
}

/** Applies one edit to the selected rule (refused while loading, failed or saving). */
function edit(change, structural = false) {
  const rule = selectedRule();
  if (!rule || !editable()) return;
  change(rule);
  if (structural) renderAll();
  else {
    renderList();
    renderErrors();
    renderPreview();
    renderToolbar();
  }
}

function select(id) {
  if (selectedId === id) return;
  selectedId = id;
  disarmDelete();
  renderAll();
  // Stacked layout (small windows): bring the editor of the chosen rule into view.
  $("ar-dock").scrollIntoView?.({ block: "nearest" });
}

function toggleRule(rule) {
  if (!editable()) return;
  rule.enabled = !rule.enabled;
  renderAll();
}

function move(rule, delta) {
  if (!editable()) return;
  rules = moveRule(rules, rule.id, delta);
  renderAll();
}

function disarmDelete() {
  deleteArmedId = null;
  clearTimeout(deleteTimer);
}

function deleteRule(rule) {
  if (!editable()) return;
  if (deleteArmedId !== rule.id) {
    disarmDelete();
    deleteArmedId = rule.id;
    deleteTimer = setTimeout(() => {
      deleteArmedId = null;
      renderList();
    }, 4000);
    renderList();
    return;
  }
  disarmDelete();
  rules = rules.filter((r) => r.id !== rule.id);
  if (selectedId === rule.id) selectedId = null;
  renderAll();
}

function discard() {
  if (!editable() || togglingMaster || !isDirty()) return;
  void reloadDiscarding();
}

async function reloadDiscarding() {
  imported = false;
  baseline = "";
  loaded = false; // forces doLoad to replace the draft with the persisted rules
  await loadAll();
}

// ── Saving ──────────────────────────────────────────────────

function failingRules() {
  return validateRules(rules, templates);
}

async function save() {
  if (saving) return false;
  if (togglingMaster) {
    // The backend applies the switch change first and a save carries the switch value: saving
    // now could write the old value back over the change being confirmed. The draft is kept.
    showBanner("The Enabled switch is being changed. Save again in a moment.", "ok");
    return false;
  }
  if (!loaded || loading) {
    showBanner(unavailable("saving"), loading ? "ok" : "error", { retry: !loading });
    return false;
  }
  const problems = failingRules();
  if (problems.size > 0) {
    const firstId = rules.find((r) => problems.has(r.id)).id;
    selectedId = firstId;
    showBanner(
      `Nothing was saved: ${problems.size === 1 ? "1 rule has" : `${problems.size} rules have`} invalid settings (marked “!”). Fix them and save again.`,
      "error",
    );
    renderAll();
    $("ar-dock").scrollIntoView?.({ block: "nearest" });
    return false;
  }

  // The draft is locked while saving, so what is marked saved is exactly what was written.
  const snapshot = snapshotRules(rules);
  const config = { enabled, rules: rules.map(ruleToPersisted) };
  const wasImported = imported;
  saving = true;
  renderAll();
  try {
    await invoke("save_auto_reply", { config });
  } catch (err) {
    saving = false;
    logError(`Failed to save auto reply rules: ${String(err)}`, "autoreply.js:save");
    showBanner(`Could not save: ${String(err)}. Your edits are kept and the previous rules stay active.`, "error");
    renderAll();
    return false;
  }
  saving = false;
  baseline = snapshot;
  imported = false;
  let note = "";
  if (wasImported) {
    try {
      removeLegacySettings(window.localStorage ?? globalThis.localStorage);
    } catch (err) {
      logError(`Could not remove the previous settings: ${String(err)}`, "autoreply.js:legacy");
      note = ` The previous settings could not be removed (${String(err)}).`;
    }
  }
  showBanner(`Saved ${rules.length} rule(s) at ${new Date().toLocaleTimeString()}.${note}`, note ? "warn" : "ok");
  renderAll();
  return true;
}

// ── Master switch ───────────────────────────────────────────

/** Persists ONLY the enabled flag: the rules being edited are neither saved nor applied. */
async function toggleMaster() {
  const want = $("ar-enabled").checked;
  if (!loaded || loading || saving || togglingMaster) {
    $("ar-enabled").checked = enabled;
    return;
  }
  togglingMaster = true;
  renderToolbar();
  try {
    await invoke("set_auto_reply_enabled", { enabled: want });
    enabled = want;
    hideBannerIfOk();
  } catch (err) {
    logError(`Failed to change the auto reply switch: ${String(err)}`, "autoreply.js:toggle");
    showBanner(`Could not ${want ? "enable" : "disable"} automatic replies: ${String(err)}`, "error");
  } finally {
    togglingMaster = false;
  }
  renderToolbar();
}

// ── Dirty guard ─────────────────────────────────────────────

let pendingAction = null;

function askBeforeLeaving(proceed) {
  pendingAction = proceed;
  $("ar-guard").classList.remove("hidden");
  renderToolbar();
}

function closeGuard() {
  pendingAction = null;
  $("ar-guard").classList.add("hidden");
}

function guardCancel() {
  if (saving) return;
  closeGuard();
}

async function guardSave() {
  if (saving || !pendingAction) return;
  if (togglingMaster) {
    showBanner("The Enabled switch is being changed. Save again in a moment.", "ok");
    return;
  }
  const action = pendingAction;
  if (await save()) {
    closeGuard();
    action();
  }
}

function guardDiscard() {
  if (saving || togglingMaster || !pendingAction) return;
  const action = pendingAction;
  closeGuard();
  imported = false;
  rules = [];
  baseline = snapshotRules([]);
  selectedId = null;
  loaded = false;
  action();
  void loadAll();
}

// ── Banner ──────────────────────────────────────────────────

function showBanner(text, kind = "ok", { retry = false } = {}) {
  const banner = $("ar-banner");
  banner.classList.remove("hidden");
  banner.classList.toggle("error", kind === "error");
  banner.classList.toggle("warn", kind === "warn");
  $("ar-banner-text").textContent = text;
  $("ar-banner-retry").classList.toggle("hidden", !retry);
}

function hideBanner() {
  $("ar-banner").classList.add("hidden");
}

/** A successful switch change clears an older error but keeps the import notice. */
function hideBannerIfOk() {
  if (!imported) hideBanner();
}

// ── Rendering ───────────────────────────────────────────────

function renderAll() {
  renderToolbar();
  renderList();
  renderForm();
}

function renderToolbar() {
  const dirty = isDirty();
  $("ar-dirty").classList.toggle("hidden", !dirty);
  $("ar-saved").classList.toggle("hidden", dirty || !loaded);
  $("ar-save").disabled = !loaded || loading || saving || togglingMaster || !dirty;
  $("ar-discard").disabled = !loaded || loading || saving || togglingMaster || !dirty;
  $("ar-add").disabled = !editable();
  $("ar-enabled").checked = enabled;
  $("ar-enabled").disabled = !loaded || loading || saving || togglingMaster;
  $("ar-guard-save").disabled = saving || togglingMaster;
  $("ar-guard-discard").disabled = saving || togglingMaster;
  $("ar-guard-cancel").disabled = saving;
  // The badge shows what the backend applies, never the draft or an unconfirmed toggle.
  $("autoreply-badge").classList.toggle("hidden", !(loaded && enabled));
}

function renderList() {
  const list = $("ar-list");
  list.replaceChildren();
  const active = rules.filter((r) => r.enabled).length;
  $("ar-count").textContent = loaded
    ? `${rules.length === 1 ? "1 rule" : `${rules.length} rules`} · ${active} enabled · evaluated top to bottom`
    : "";
  $("ar-empty").classList.toggle("hidden", !(loaded && rules.length === 0));
  if (!loaded) {
    list.appendChild(el("p", "empty-note", loading ? "Loading rules…" : "Rules unavailable."));
    return;
  }
  const problems = failingRules();
  rules.forEach((rule, index) => {
    const row = el("div", `ar-row ar-item${rule.id === selectedId ? " on" : ""}${rule.enabled ? "" : " off"}`);
    row.setAttribute("role", "listitem");
    row.tabIndex = 0;
    row.addEventListener("click", () => select(rule.id));
    row.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" && ev.target === row) select(rule.id);
    });

    const on = el("span", "ar-c ar-c-on");
    const box = el("input");
    box.type = "checkbox";
    box.className = "ar-on";
    box.checked = rule.enabled;
    box.disabled = !editable();
    box.setAttribute("aria-label", `Enable rule ${rule.name}`);
    box.addEventListener("click", (ev) => ev.stopPropagation?.());
    box.addEventListener("change", () => toggleRule(rule));
    on.appendChild(box);
    row.appendChild(on);

    const when = el("span", "ar-c ar-c-when");
    when.appendChild(el("b", "", rule.name.trim() || "Untitled"));
    when.appendChild(el("span", "ar-sub", describeTrigger(rule)));
    if (problems.has(rule.id)) {
      const bad = el("span", "ar-bad", "!");
      bad.title = problems.get(rule.id)[0].message;
      when.appendChild(bad);
    }
    row.appendChild(when);
    row.appendChild(el("span", "ar-c ar-c-cond", describeCondition(rule)));
    row.appendChild(el("span", "ar-c ar-c-reply", describeAction(rule, templates)));
    row.appendChild(el("span", "ar-c ar-c-delay", describeDelay(rule)));

    const tools = el("span", "ar-c ar-c-tools");
    const mk = (label, title, onClick, disabled, extra = "") => {
      const b = el("button", `btn sm gh ${extra}`.trim(), label);
      b.type = "button";
      b.title = title;
      b.disabled = disabled || !editable();
      b.setAttribute("aria-label", `${title}: ${rule.name}`);
      b.addEventListener("click", (ev) => {
        ev.stopPropagation?.();
        onClick();
      });
      tools.appendChild(b);
    };
    mk("↑", "Move up (higher priority)", () => move(rule, -1), index === 0);
    mk("↓", "Move down", () => move(rule, 1), index === rules.length - 1);
    const armed = deleteArmedId === rule.id;
    mk(armed ? "Sure?" : "✕", armed ? "Click again to delete" : "Delete rule", () => deleteRule(rule), false, "dg");
    row.appendChild(tools);
    list.appendChild(row);
  });
}

function renderForm() {
  const rule = selectedRule();
  $("ar-dock-empty").classList.toggle("hidden", rule !== null);
  $("ar-form").classList.toggle("hidden", rule === null);
  if (!rule) return;

  const set = (id, value) => {
    if ($(id).value !== value) $(id).value = value;
  };
  set("ar-name", rule.name);
  $("ar-rule-enabled").checked = rule.enabled;
  set("ar-trigger", rule.trigger);
  set("ar-type", rule.messageType);
  $("ar-cond-use").checked = rule.useCondition;
  set("ar-cond-seg", rule.condSegment);
  set("ar-cond-field", rule.condField);
  set("ar-cond-op", rule.condOperator);
  set("ar-cond-value", rule.condValue);
  set("ar-action", rule.action);
  set("ar-literal", rule.literal);
  set("ar-ack-type", rule.ackType);
  set("ar-ack-code", rule.ackCode);
  set("ar-delay", rule.delay);

  const hl7 = rule.trigger === "hl7";
  $("ar-hl7-fields").classList.toggle("hidden", !hl7);
  $("ar-cond-fields").classList.toggle("hidden", !(hl7 && rule.useCondition));
  $("ar-act-template").classList.toggle("hidden", rule.action !== "template");
  $("ar-act-literal").classList.toggle("hidden", rule.action !== "literal");
  $("ar-act-ack").classList.toggle("hidden", rule.action !== "hl7_ack");
  $("ar-act-none").classList.toggle("hidden", rule.action !== "none");
  $("ar-delay-field").classList.toggle("hidden", rule.action === "none");
  renderTemplateOptions(rule);

  const locked = !editable();
  for (const id of [
    "ar-name", "ar-rule-enabled", "ar-trigger", "ar-type", "ar-cond-use", "ar-cond-seg",
    "ar-cond-field", "ar-cond-op", "ar-cond-value", "ar-action", "ar-template",
    "ar-literal", "ar-ack-type", "ar-ack-code", "ar-delay",
  ]) {
    $(id).disabled = locked;
  }
  renderErrors();
  renderPreview();
}

/** Options come ONLY from the saved library (never from an unsaved template draft). */
function renderTemplateOptions(rule) {
  const select = $("ar-template");
  select.replaceChildren();
  const add = (value, label, disabled = false) => {
    const option = el("option", "", label);
    option.value = value;
    option.disabled = disabled;
    select.appendChild(option);
  };
  add("", templates ? "Choose a template…" : "Template library unavailable", true);
  for (const template of templates ?? []) add(template.id, template.name);
  if (rule.templateId && !(templates ?? []).some((t) => t.id === rule.templateId)) {
    add(rule.templateId, templates ? `(missing) ${rule.templateId}` : `(unavailable) ${rule.templateId}`);
  }
  select.value = rule.templateId;
}

function renderErrors() {
  const rule = selectedRule();
  const box = $("ar-errors");
  box.replaceChildren();
  const errors = rule ? validateRule(rule, templates) : [];
  box.classList.toggle("hidden", errors.length === 0);
  for (const error of errors) box.appendChild(el("li", "", error.message));
  for (const [id, field] of [
    ["ar-name", "name"], ["ar-type", "messageType"], ["ar-cond-seg", "condSegment"],
    ["ar-cond-field", "condField"], ["ar-cond-value", "condValue"], ["ar-template", "templateId"],
    ["ar-literal", "literal"], ["ar-ack-type", "ackType"], ["ar-ack-code", "ackCode"],
    ["ar-delay", "delay"],
  ]) {
    $(id).setAttribute("aria-invalid", errors.some((e) => e.field === field) ? "true" : "false");
  }
}

function renderPreview() {
  const rule = selectedRule();
  const pre = $("ar-preview");
  const vars = $("ar-vars");
  pre.replaceChildren();
  vars.replaceChildren();
  const template =
    rule && rule.action === "template" && templates
      ? templates.find((t) => t.id === rule.templateId)
      : null;
  $("ar-preview-box").classList.toggle("hidden", !template);
  if (!template) return;
  const { text } = previewTemplate(template, rule.trigger === "hl7");
  renderTokens(pre, text, { breakAfterCr: true, segments: true });
  for (const v of variableSources(template)) {
    vars.appendChild(el("li", "", `{{${v.name}}} — ${v.source}`));
  }
}
