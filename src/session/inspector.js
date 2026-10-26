// Docked message inspector: raw / parsed / hex views of the selected message.
import {
  describeMessage,
  toHexDump,
  hasRawBytes,
  hl7Ack,
  parseAstmTransfer,
  HL7_SEGMENT_NAMES,
  ASTM_RECORD_NAMES,
} from "./inspector-core.js";
import { partitionFrames } from "./astm-group.js";
import { badge, replySummary, transferBadges } from "./astm-view.js";
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
  if (info.kind === "hl7" || info.transfer) tabs.push(["parsed", "Parsed"]);
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

/** Redraws the inspector if it shows this (updated in place) message. */
export function refreshInspector(rec) {
  const state = states.get(rec.session_id);
  if (!state || state.record !== rec) return;
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
    if (info.transfer) {
      const frames = info.transfer.entries.filter((entry) => entry.kind === "frame").length;
      meta.appendChild(el("span", "", `${frames} frame${frames === 1 ? "" : "s"}`));
      const summary = replySummary(info.transfer);
      if (summary) meta.appendChild(badge(summary.text, summary.tone));
      for (const node of transferBadges(info.transfer)) meta.appendChild(node);
      if (info.transfer.status === "open") meta.appendChild(badge("in progress", "mute"));
    }
  }

  for (const [id, label] of tabs) {
    const button = cloneTemplate(INSPECTOR_TAB, { label });
    button.dataset.tab = id;
    if (id === tab) button.classList.add("on");
    button.setAttribute("aria-selected", String(id === tab));
    tabsEl.appendChild(button);
  }

  if (tab === "parsed" && info.transfer) renderParsedAstm(body, info.transfer);
  else if (tab === "parsed") renderParsed(body, info.hl7);
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
  renderSegments(body, hl7.segments, HL7_SEGMENT_NAMES, flagged);

  body.appendChild(el("small", "", "Raw"));
  const raw = el("div", "raw");
  renderTokens(raw, record.content, { breakAfterCr: true, segments: true });
  body.appendChild(raw);
}

/** ASTM message: records of the acknowledged frames, then the frames kept apart. */
function renderParsedAstm(body, transfer) {
  const partition = partitionFrames(transfer);
  const { merged, apart } = parseAstmTransfer(partition);
  if (merged) renderSegments(body, merged.segments, ASTM_RECORD_NAMES, new Set());
  else body.appendChild(el("p", "", "No frame to display yet."));

  if (merged) {
    body.appendChild(el("small", "", "Message"));
    body.appendChild(el("div", "raw", merged.segments.map((segment) => segment.raw).join("\n")));
  }

  if (apart.length > 0) {
    body.appendChild(el("small", "", "Not acknowledged"));
    for (const { entry, parsed } of apart) {
      const label = entry.frame.number === null ? "Frame" : `Frame #${entry.frame.number}`;
      const reply = entry.reply ? entry.reply.kind.toUpperCase() : "no reply";
      body.appendChild(el("small", "", `${label} · ${reply}`));
      if (parsed) renderSegments(body, parsed.segments, ASTM_RECORD_NAMES, new Set());
    }
  }
}

function renderSegments(body, segments, names, flagged) {
  for (const segment of segments) {
    if (segment.name === null) {
      const row = el("div", "sgh bad");
      row.appendChild(el("span", "ar", "·"));
      row.appendChild(el("em", "", `(unparsed) ${segment.raw}`));
      body.appendChild(row);
      continue;
    }
    const description = names[segment.name];
    const isCollapsed = collapsed.has(segment.name);
    const block = cloneTemplate(SEGMENT_BLOCK, {
      arrow: isCollapsed ? "▸" : "▾",
      name: segment.name,
      ...(description && { description }),
    });
    const head = block.querySelector(".sgh");
    head.dataset.segment = segment.name;
    head.setAttribute("aria-expanded", String(!isCollapsed));
    if (isCollapsed) block.classList.add("collapsed");
    if (!description) block.querySelector(".sg-desc").remove();
    for (const field of segment.fields) {
      const row = el("div", flagged.has(field.id) ? "fl hl" : "fl");
      row.appendChild(el("span", "", field.id));
      row.appendChild(el("span", "", field.value));
      row.title = field.label ? `${field.id} · ${field.label}` : field.id;
      block.appendChild(row);
    }
    body.appendChild(block);
  }
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
      head.querySelector(".sg-arrow").textContent = isCollapsed ? "▸" : "▾";
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
