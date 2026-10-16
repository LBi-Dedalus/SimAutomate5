// Docked message inspector: raw / parsed / hex views of the selected message.
import {
  describeMessage,
  toHexDump,
  hasRawBytes,
  hl7Ack,
  HL7_SEGMENT_NAMES,
} from "./inspector-core.js";
import { renderTokens, el } from "../render.js";
import { logError } from "../log.js";
import { cloneTemplate } from "../include.js";

const INSPECTOR_TAB = "session/inspector-tab.html";
const SEGMENT_BLOCK = "session/segment-block.html";

export const SAVE_AS_TEMPLATE_EVENT = "simautomate:save-as-template";

// One inspected message / tab / collapsed-segments state per session: switching session
// restores what was being inspected there and never shows another session's message.
const states = new Map();
let currentKey = null;
// The state of the displayed session (a throw-away one while no session is open).
let view = newState();

function newState() {
  return { record: null, tab: null, collapsed: new Set() };
}

function stateFor(key) {
  if (!states.has(key)) states.set(key, newState());
  return states.get(key);
}

// Aliases kept for the render code below.
let record = null;
let tab = null;
let collapsed = view.collapsed;

function sync() {
  record = view.record;
  tab = view.tab;
  collapsed = view.collapsed;
}

function available(rec) {
  const info = describeMessage(rec);
  const tabs = [];
  if (info.kind === "hl7") tabs.push(["parsed", "Parsed"]);
  tabs.push(["raw", "Raw"]);
  if (hasRawBytes(rec)) tabs.push(["hex", "Hex"]);
  return { info, tabs };
}

/** Displays the inspector state of a session (null: no session open). */
export function activateInspector(sessionId) {
  currentKey = sessionId;
  view = sessionId === null ? newState() : stateFor(sessionId);
  sync();
  render();
}

/** Inspects a message of its own session; shown only if that session is displayed. */
export function showInspector(rec) {
  const state = stateFor(rec.session_id);
  state.record = rec;
  const { tabs } = available(rec);
  if (!tabs.some(([id]) => id === state.tab)) state.tab = tabs[0][0];
  if (rec.session_id === currentKey) {
    view = state;
    sync();
    render();
  }
}

export function clearInspector(sessionId) {
  const state = stateFor(sessionId);
  state.record = null;
  state.tab = null;
  if (sessionId === currentKey) {
    view = state;
    sync();
    render();
  }
}

/** Drops everything remembered about a closed session. */
export function forgetInspector(sessionId) {
  states.delete(sessionId);
}

function render() {
  const title = document.getElementById("insp-title");
  const dir = document.getElementById("insp-dir");
  const meta = document.getElementById("insp-meta");
  const tabsEl = document.getElementById("insp-tabs");
  const body = document.getElementById("insp-body");
  const copy = document.getElementById("insp-copy");
  const save = document.getElementById("insp-save-template");

  tabsEl.replaceChildren();
  body.replaceChildren();
  meta.replaceChildren();

  if (!record) {
    title.textContent = "Message inspector";
    dir.classList.add("hidden");
    body.appendChild(
      el("p", "", "Select a message in the conversation to inspect it."),
    );
    copy.disabled = true;
    save.disabled = true;
    return;
  }

  const { info, tabs } = available(record);
  title.textContent = info.title;
  dir.className = "badge";
  dir.textContent = { sent: "OUT", received: "IN" }[record.msg_type] ?? "SYS";
  if (record.msg_type === "received") dir.classList.add("in");
  if (record.msg_type === "systemerror") dir.classList.add("error");
  if (record.msg_type === "systemwarn") dir.classList.add("warn");

  const time = new Date(record.timestamp);
  meta.appendChild(
    el("span", "", Number.isNaN(time.getTime()) ? String(record.timestamp) : time.toLocaleTimeString()),
  );
  if (hasRawBytes(record)) meta.appendChild(el("span", "", `${record.raw.length} bytes`));
  if (info.kind === "hl7") {
    meta.appendChild(el("span", "", info.hl7.version ? `HL7 ${info.hl7.version}` : "HL7"));
    if (info.hl7.controlId) meta.appendChild(el("span", "", `#${info.hl7.controlId}`));
  } else if (info.kind === "astm") {
    meta.appendChild(el("span", "", "ASTM"));
  }

  for (const [id, label] of tabs) {
    const button = cloneTemplate(INSPECTOR_TAB, { label });
    button.dataset.tab = id;
    if (id === tab) button.classList.add("on");
    button.setAttribute("aria-selected", String(id === tab));
    tabsEl.appendChild(button);
  }

  if (tab === "parsed") renderParsed(body, info.hl7);
  else if (tab === "hex") renderHex(body);
  else renderRaw(body);

  copy.disabled = false;
  save.disabled = record.msg_type !== "sent" && record.msg_type !== "received";
}

function renderRaw(body) {
  const raw = el("div", "raw");
  renderTokens(raw, record.content, { breakAfterCr: true, segments: true });
  body.appendChild(raw);
}

function renderHex(body) {
  body.appendChild(el("pre", "raw hex", toHexDump(record.raw)));
}

function renderParsed(body, hl7) {
  const ack = hl7Ack(hl7);
  const flagged = new Set(ack && !ack.ok ? ["MSA-1", "MSA-3"] : []);

  for (const segment of hl7.segments) {
    if (segment.name === null) {
      const row = el("div", "sgh bad");
      row.appendChild(el("span", "ar", "·"));
      row.appendChild(el("em", "", `(unparsed) ${segment.raw}`));
      body.appendChild(row);
      continue;
    }
    const description = HL7_SEGMENT_NAMES[segment.name];
    const isCollapsed = collapsed.has(segment.name);
    const block = cloneTemplate(SEGMENT_BLOCK, {
      arrow: isCollapsed ? "▸" : "▾",
      name: segment.name,
      ...(description && { description }),
    });
    const head = block.querySelector('[data-slot="head"]');
    head.dataset.segment = segment.name;
    head.setAttribute("aria-expanded", String(!isCollapsed));
    if (isCollapsed) block.classList.add("collapsed");
    if (!description) block.querySelector('[data-slot="description"]').remove();
    for (const field of segment.fields) {
      const row = el("div", flagged.has(field.id) ? "fl hl" : "fl");
      row.appendChild(el("span", "", field.id));
      row.appendChild(el("span", "", field.value));
      row.title = field.label ? `${field.id} · ${field.label}` : field.id;
      block.appendChild(row);
    }
    body.appendChild(block);
  }

  body.appendChild(el("small", "", "Raw"));
  const raw = el("div", "raw");
  renderTokens(raw, record.content, { breakAfterCr: true, segments: true });
  body.appendChild(raw);
}

function init() {
  window.actions ??= {};
  window.actions.insp = {
    tab(_ev, button) {
      view.tab = button.dataset.tab;
      sync();
      render();
    },
    toggleSegment(_ev, head) {
      const block = head.closest(".seg-block");
      const isCollapsed = block.classList.toggle("collapsed");
      if (isCollapsed) collapsed.add(head.dataset.segment);
      else collapsed.delete(head.dataset.segment);
      head.querySelector('[data-slot="arrow"]').textContent = isCollapsed ? "▸" : "▾";
      head.setAttribute("aria-expanded", String(!isCollapsed));
    },
    async copy() {
      if (!record) return;
      try {
        await navigator.clipboard.writeText(record.content);
      } catch (err) {
        logError(`Copy failed: ${String(err)}`, "inspector.js:copy");
        const meta = document.getElementById("insp-meta");
        meta.appendChild(el("span", "badge error", "Copy failed"));
      }
    },
    saveTemplate() {
      if (!record) return;
      document.dispatchEvent(
        new CustomEvent(SAVE_AS_TEMPLATE_EVENT, {
          detail: { payload: record.content },
        }),
      );
    },
  };
}

init();
