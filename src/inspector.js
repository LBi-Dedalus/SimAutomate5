// Docked message inspector: raw / parsed / hex views of the selected message.
import {
  describeMessage,
  toHexDump,
  hasRawBytes,
  hl7Ack,
  HL7_SEGMENT_NAMES,
} from "./inspector-core.js";
import { renderTokens, el } from "./render.js";
import { logError } from "./log.js";

export const SAVE_AS_TEMPLATE_EVENT = "simautomate:save-as-template";

let record = null;
let tab = null;
/** Segment names the user collapsed; kept while browsing messages. */
const collapsed = new Set();

function available(rec) {
  const info = describeMessage(rec);
  const tabs = [];
  if (info.kind === "hl7") tabs.push(["parsed", "Parsed"]);
  tabs.push(["raw", "Raw"]);
  if (hasRawBytes(rec)) tabs.push(["hex", "Hex"]);
  return { info, tabs };
}

export function showInspector(rec) {
  record = rec;
  const { tabs } = available(rec);
  if (!tabs.some(([id]) => id === tab)) tab = tabs[0][0];
  render();
}

export function clearInspector() {
  record = null;
  tab = null;
  render();
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
      el("p", "empty-note", "Select a message in the conversation to inspect it."),
    );
    copy.disabled = true;
    save.disabled = true;
    return;
  }

  const { info, tabs } = available(record);
  title.textContent = info.title;
  dir.className = "mbadge";
  dir.textContent = { sent: "OUT", received: "IN" }[record.msg_type] ?? "SYS";
  if (record.msg_type === "received") dir.classList.add("in");
  if (record.msg_type === "systemerror") dir.classList.add("err");
  if (record.msg_type === "systemwarn") dir.classList.add("warnb");

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
    const button = el("button", id === tab ? "on" : "", label);
    button.type = "button";
    button.setAttribute("role", "tab");
    button.setAttribute("aria-selected", String(id === tab));
    button.addEventListener("click", () => {
      tab = id;
      render();
    });
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
  body.appendChild(
    el("p", "hint", "Exact bytes of this event as written to / read from the socket."),
  );
}

function renderParsed(body, hl7) {
  const summary = el("div", "kv");
  for (const [label, value] of [
    ["Type", hl7.messageType],
    ["Control ID", hl7.controlId],
    ["Version", hl7.version],
  ]) {
    const cell = el("div");
    cell.appendChild(el("small", "", label));
    cell.appendChild(el("b", "", value || "—"));
    summary.appendChild(cell);
  }
  body.appendChild(summary);

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
    const block = el("div", "seg-block");
    if (collapsed.has(segment.name)) block.classList.add("collapsed");
    const head = el("button", "sgh");
    head.type = "button";
    head.setAttribute("aria-expanded", String(!block.classList.contains("collapsed")));
    const arrow = el("span", "ar", block.classList.contains("collapsed") ? "▸" : "▾");
    head.appendChild(arrow);
    head.appendChild(el("span", "nm", segment.name));
    const description = HL7_SEGMENT_NAMES[segment.name];
    if (description) head.appendChild(el("em", "", description));
    head.addEventListener("click", () => {
      const isCollapsed = block.classList.toggle("collapsed");
      if (isCollapsed) collapsed.add(segment.name);
      else collapsed.delete(segment.name);
      arrow.textContent = isCollapsed ? "▸" : "▾";
      head.setAttribute("aria-expanded", String(!isCollapsed));
    });
    block.appendChild(head);
    for (const field of segment.fields) {
      const row = el("div", flagged.has(field.id) ? "fl hl" : "fl");
      row.appendChild(el("span", "", field.id));
      row.appendChild(el("span", "", field.label));
      row.appendChild(el("span", "", field.value));
      row.title = field.label ? `${field.id} · ${field.label}` : field.id;
      block.appendChild(row);
    }
    body.appendChild(block);
  }

  body.appendChild(el("span", "lbl", "Raw"));
  const raw = el("div", "raw");
  renderTokens(raw, record.content, { breakAfterCr: true, segments: true });
  body.appendChild(raw);

  body.appendChild(
    el(
      "p",
      "hint",
      "Best effort, based on this single event: a message split over several reads, or several messages in one read, is not reassembled.",
    ),
  );
}

document.addEventListener("DOMContentLoaded", () => {
  document.getElementById("insp-copy").addEventListener("click", async () => {
    if (!record) return;
    try {
      await navigator.clipboard.writeText(record.content);
    } catch (err) {
      logError(`Copy failed: ${String(err)}`, "inspector.js:copy");
      const meta = document.getElementById("insp-meta");
      meta.appendChild(el("span", "inline-error", "Copy failed"));
    }
  });

  document.getElementById("insp-save-template").addEventListener("click", () => {
    if (!record) return;
    document.dispatchEvent(
      new CustomEvent(SAVE_AS_TEMPLATE_EVENT, {
        detail: { payload: record.content },
      }),
    );
  });
});
