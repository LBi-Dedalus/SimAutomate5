// Auto reply view: ordered, global rules (first enabled match wins) with a docked rule editor.
// Everything is configured in the rules; there is no default acknowledgement.
//
// Persistence goes through the backend config file (load_config / save_auto_reply /
// set_auto_reply_enabled); the backend is the single source of truth and every command returns
// the effective config. The master switch persists ONLY the enabled flag, never the draft.
// A template save also returns the effective auto reply: it arrives through autoreply-store.js.
// Template choices come from the saved template library (template-store.js), never from drafts.
import {
  describeAction,
  describeCondition,
  describeDelay,
  describeTrigger,
  moveRule,
  newRule,
  previewTemplate,
  ruleFromPersisted,
  ruleToPersisted,
  snapshotRules,
  validateRule,
  validateRules,
  variableSources,
} from "./autoreply-core.js";
import { getSavedTemplates, subscribeSavedTemplates } from "../templates/template-store.js";
import { subscribeAutoReply } from "./autoreply-store.js";
import { setLeaveGuard } from "../nav.js";
import { el, renderTokens } from "../render.js";
import { cloneTemplate } from "../include.js";
import { logError } from "../log.js";

const RULE_ROW = "autoreply/rule-row.html";

const { invoke } = window.__TAURI__.core;

/** Draft rules (editing model, in priority order). */
let rules = [];
/** Snapshot of the persisted rules; the draft is dirty when it differs. */
let baseline = snapshotRules([]);
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
/**
 * Revision of the configuration last applied here. Backend revisions grow with every save, in
 * lock order, but IPC responses may be delivered in any order: anything not newer is ignored.
 */
let confirmedRevision = -1;
/** Highest-revision confirmation published while an operation was running. */
let pendingConfirmed = null;

/**
 * Ends an operation: the operation's own result and the publications buffered meanwhile are all
 * reconciled; the revision guard keeps only what is newest, whatever their arrival order.
 */
function settleConfirmed(result) {
  const buffered = pendingConfirmed;
  pendingConfirmed = null;
  for (const config of [result, buffered]) {
    if (config) reconcileConfirmed(config);
  }
}

const $ = (id) => document.getElementById(id);

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

  subscribeAutoReply(applyConfirmed);

  await loadAll();
}

/**
 * The backend confirmed a new effective auto reply (e.g. returned by a template save): the
 * switch and the saved-rules baseline follow it. An unsaved draft is never replaced.
 */
function applyConfirmed(config) {
  if (loading || saving || togglingMaster) {
    // Not dropped: the newest one is reconciled when the running operation finishes.
    if (!pendingConfirmed || config.revision > pendingConfirmed.revision) pendingConfirmed = config;
    return;
  }
  reconcileConfirmed(config);
}

/**
 * Reconciles a backend-confirmed { revision, enabled, rules }: replaces visible rules only if the
 * draft is clean. A config that is not newer than the last applied one is ignored.
 */
function reconcileConfirmed(config) {
  if (!loaded || config.revision <= confirmedRevision) return;
  confirmedRevision = config.revision;
  const wasDirty = isDirty();
  enabled = Boolean(config.enabled);
  const persisted = config.rules.map(ruleFromPersisted);
  baseline = snapshotRules(persisted);
  if (!wasDirty) {
    rules = persisted;
    if (selectedId && !selectedRule()) selectedId = null;
  }
  renderAll();
}

// ── State helpers ───────────────────────────────────────────

function isDirty() {
  return loaded && snapshotRules(rules) !== baseline;
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
  try {
    const { revision, auto_reply: persistedConfig } = await invoke("load_config");
    ok = true;
    if (loaded && revision < confirmedRevision) {
      // Stale response (a newer configuration was confirmed meanwhile): nothing to apply.
    } else {
      confirmedRevision = Math.max(confirmedRevision, revision);
      enabled = Boolean(persistedConfig.enabled);
      if (isDirty() && loaded) {
        // A reload never replaces edits in progress.
      } else {
        rules = persistedConfig.rules.map(ruleFromPersisted);
        baseline = snapshotRules(rules);
      }
    }
    loadError = null;
    loaded = true;
  } catch (err) {
    loadError = String(err);
    loaded = false;
    enabled = false;
    rules = [];
    baseline = snapshotRules([]);
    selectedId = null;
    logError(`Failed to load auto reply rules: ${loadError}`, "autoreply.js:load");
  } finally {
    loading = false;
  }
  if (ok) settleConfirmed(null);
  else pendingConfirmed = null;
  if (!ok) {
    showBanner(
      `Could not load the auto reply rules: ${loadError}. Automatic replies are off and saving is disabled so the file is not overwritten.`,
      "error",
      { retry: true },
    );
  } else {
    hideBanner();
  }
  if (selectedId && !selectedRule()) selectedId = null;
  renderAll();
  return ok;
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
  saving = true;
  renderAll();
  let result;
  try {
    result = await invoke("save_auto_reply", { config });
  } catch (err) {
    saving = false;
    settleConfirmed(null);
    logError(`Failed to save auto reply rules: ${String(err)}`, "autoreply.js:save");
    showBanner(`Could not save: ${String(err)}. Your edits are kept and the previous rules stay active.`, "error");
    renderAll();
    return false;
  }
  saving = false;
  baseline = snapshot; // the draft was locked: it is exactly what was written, so it is clean
  settleConfirmed({ revision: result.revision, ...result.auto_reply });
  hideBanner();
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
    const result = await invoke("set_auto_reply_enabled", { enabled: want });
    togglingMaster = false;
    settleConfirmed({ revision: result.revision, ...result.auto_reply });
    hideBanner();
  } catch (err) {
    togglingMaster = false;
    logError(`Failed to change the auto reply switch: ${String(err)}`, "autoreply.js:toggle");
    showBanner(`Could not ${want ? "enable" : "disable"} automatic replies: ${String(err)}`, "error");
    settleConfirmed(null);
  }
  renderAll();
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

// ── Rendering ───────────────────────────────────────────────

function renderAll() {
  renderToolbar();
  renderList();
  renderForm();
}

function renderToolbar() {
  const dirty = isDirty();
  $("ar-dirty").classList.toggle("hidden", !dirty);
  $("view-autoreply").classList.toggle("pending", dirty);
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
  renderStatus();
}

/** Status strip: the confirmed master state, like the sidebar badge. */
function renderStatus() {
  const on = loaded && enabled;
  $("ar-strip").classList.toggle("is-off", !on);
  $("ar-state-badge").className = `badge ${on ? "ok" : "mute"}`;
  $("ar-state-badge").textContent = on ? "● ON" : loaded ? "● OFF" : "● —";
  $("ar-state-text").textContent = on
    ? "Automatic replies are active"
    : loaded
      ? "Automatic replies are paused"
      : loading
        ? "Loading automatic replies…"
        : "Automatic replies unavailable";
}

function renderList() {
  const list = $("ar-list");
  list.replaceChildren();
  if (!loaded) {
    list.appendChild(el("p", "", loading ? "Loading rules…" : "Rules unavailable."));
    return;
  }
  const problems = failingRules();
  rules.forEach((rule, index) => {
    const armed = deleteArmedId === rule.id;
    const row = cloneTemplate(RULE_ROW, {
      name: rule.name.trim() || "Untitled",
      trigger: describeTrigger(rule),
      condition: describeCondition(rule),
      action: describeAction(rule, templates),
      delay: describeDelay(rule),
      remove: armed ? "Sure?" : "✕",
    });
    const part = (cls) => row.querySelector(`.${cls}`);
    row.dataset.id = rule.id;
    row.classList.toggle("on", rule.id === selectedId);
    row.classList.toggle("off", !rule.enabled);

    const box = part("ar-check");
    box.checked = rule.enabled;
    box.disabled = !editable();
    box.setAttribute("aria-label", `Enable rule ${rule.name}`);

    const bad = part("ar-bad");
    if (problems.has(rule.id)) bad.title = problems.get(rule.id)[0].message;
    else bad.remove();

    const tool = (name, title, disabled) => {
      const b = part(name);
      b.title = title;
      b.disabled = disabled || !editable();
      b.setAttribute("aria-label", `${title}: ${rule.name}`);
    };
    tool("ar-up", "Move up (higher priority)", index === 0);
    tool("ar-down", "Move down", index === rules.length - 1);
    tool("ar-remove", armed ? "Click again to delete" : "Delete rule", false);
    list.appendChild(row);
  });
}

/** Handlers of the data-action attributes of rule-row.html; the rule is found from the row's data-id. */
const ruleOf = (node) => rules.find((r) => r.id === node.closest("[data-id]")?.dataset.id);
/** A button inside the row: the click must not also select the row. */
const own = (act) => (ev, node) => {
  ev.stopPropagation();
  const rule = ruleOf(node);
  if (rule) act(rule);
};
window.actions.ar = {
  select: (ev, node) => {
    const rule = ruleOf(node);
    if (rule) select(rule.id);
  },
  rowKey: (ev, node) => {
    if (ev.key === "Enter" && ev.target === node) window.actions.ar.select(ev, node);
  },
  toggle: (ev, node) => {
    const rule = ruleOf(node);
    if (rule) toggleRule(rule);
  },
  up: own((rule) => move(rule, -1)),
  down: own((rule) => move(rule, 1)),
  remove: own(deleteRule),
  add: addRule,
  save: () => save(),
  discard,
  master: () => toggleMaster(),
  bannerClose: hideBanner,
  retry: () => loadAll(),
  guardSave: () => guardSave(),
  guardDiscard,
  guardCancel,
  noSubmit: (ev) => ev.preventDefault(),
  // Rule form fields: data-field names the rule property.
  text: (ev, el) => edit((r) => (r[el.dataset.field] = el.value)),
  choose: (ev, el) => edit((r) => (r[el.dataset.field] = el.value), true),
  check: (ev, el) => edit((r) => (r[el.dataset.field] = el.checked), true),
};

function renderForm() {
  const rule = selectedRule();
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

await init();
