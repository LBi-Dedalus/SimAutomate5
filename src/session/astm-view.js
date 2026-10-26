// DOM rendering of an ASTM transfer in the conversation bubble: one row per entry (ENQ, frames,
// EOT). Every piece comes from a template (session/astm-*.html): this file only clones, fills
// the slots and appends.
import { el, renderTokens } from "../render.js";
import { cloneTemplate } from "../include.js";
import { replyCounts } from "./astm-group.js";

const BADGE = "session/astm-badge.html";
const FRAME = "session/astm-frame.html";

const REPLY_HINTS = {
  ack: { text: "✓ ACK", tone: "ok" },
  nak: { text: "✕ NAK", tone: "error" },
  eot: { text: "⏹ EOT", tone: "warn" },
};

/** Short summary of the answers received for the frames: { text, tone } or null. */
export function replySummary(transfer) {
  const { ack, nak, none, frames } = replyCounts(transfer);
  if (frames === 0) return null;
  const parts = [];
  if (ack) parts.push(`✓ ACK ×${ack}`);
  if (nak) parts.push(`✕ NAK ×${nak}`);
  if (none && transfer.status !== "open") parts.push(`no reply ×${none}`);
  if (parts.length === 0) return null;
  return { text: parts.join(" · "), tone: nak || (none && transfer.status !== "open") ? "error" : "ok" };
}

/** A badge; `tones` are the space-separated tone classes (ok, warn, error, mute, outline). */
export function badge(text, tones, title) {
  const node = cloneTemplate(BADGE, { text });
  node.classList.add(...tones.split(" "));
  if (title) node.title = title;
  return node;
}

/** Status badges of a transfer ("incomplete", "errors"). */
export function transferBadges(transfer) {
  const badges = [];
  if (transfer.status === "incomplete") badges.push(badge("incomplete", "warn"));
  if (transfer.hasErrors) badges.push(badge("checksum error", "error"));
  return badges;
}

function hint(reply) {
  if (!reply) return badge("∅ no reply", "mute outline");
  const { text, tone } = REPLY_HINTS[reply.kind] ?? { text: reply.kind, tone: "mute" };
  return badge(text, tone, `${reply.kind.toUpperCase()} at ${reply.timestamp}`);
}

/** One row per entry: its raw content (control characters as chips) and the answer hint beside it. */
function entryRow(entry) {
  const { frame, reply } = entry;
  const row = cloneTemplate(FRAME);
  renderTokens(row.querySelector(".af-text"), entry.content);
  const side = row.querySelector(".af-side");
  if (frame && !frame.checksum_ok) {
    side.appendChild(badge("checksum", "error", `Received ${frame.checksum}, computed ${frame.computed_checksum}`));
  }
  if (entry.kind === "frame") {
    side.appendChild(hint(reply));
    row.classList.toggle("warn", reply?.kind !== "ack");
  } else if (entry.kind === "enq") {
    side.appendChild(hint(reply));
  }
  return row;
}

/** ENQ, every frame with its raw content and the answer hint beside it, EOT. */
export function buildFrameRows(transfer) {
  const list = el("div", "vstack gap-1");
  for (const entry of transfer.entries) list.appendChild(entryRow(entry));
  return list;
}
