// Pure helpers of the template "Send to…" split button: labels, menu items, keyboard moves and
// placement. The send rule itself stays in sessions.sendTarget (passed in, never duplicated).
import { statusText } from "../session/sessions-core.js";

const OP_TEXT = { disconnecting: "Disconnecting", closing: "Closing" };

/** Label of the main button: it quick-sends to the selected session. */
export function mainLabel(active) {
  return active ? `Send to ${active.label}` : "Send";
}

/** Tooltip text of the main button. */
export function mainHint(active, canSend) {
  if (canSend) return `Send to the selected session (${active.label})`;
  return active ? "Connect the selected session to send" : "Select a connected session to send";
}

/**
 * One entry per session: `enabled` comes from `sendTarget(id)` (null: cannot send now), so the
 * menu can never offer a session the real send would refuse.
 */
export function menuItems(sessionList, activeId, sendTarget) {
  return sessionList.map((session) => ({
    id: session.id,
    label: session.label,
    state: OP_TEXT[session.op] ?? statusText(session.status),
    current: session.id === activeId,
    enabled: sendTarget(session.id) !== null,
  }));
}

/** The arrow is usable as soon as one session can send, even when the selected one cannot. */
export const hasSendable = (items) => items.some((item) => item.enabled);

/** Accessible name of a menu entry. */
export function itemText(item) {
  return `${item.label}${item.current ? " (current)" : ""}: ${item.state}`;
}

/**
 * Index of the enabled entry reached by a navigation key from `from` (-1: nothing focused),
 * wrapping around; -1 when no entry is enabled.
 */
export function moveFocus(items, from, key) {
  const enabled = items.map((item, i) => (item.enabled ? i : -1)).filter((i) => i >= 0);
  if (enabled.length === 0) return -1;
  if (key === "Home") return enabled[0];
  if (key === "End") return enabled.at(-1);
  if (key === "ArrowDown") return enabled.find((i) => i > from) ?? enabled[0];
  if (key === "ArrowUp") return [...enabled].reverse().find((i) => i < from) ?? enabled.at(-1);
  return from;
}

/** Index of the entry to focus when the menu opens: the current session if enabled, else the first enabled. */
export function initialFocus(items) {
  const current = items.findIndex((item) => item.current && item.enabled);
  return current >= 0 ? current : moveFocus(items, -1, "Home");
}

/**
 * Fixed-position placement: right-aligned with the anchor, below it when it fits, otherwise above
 * (the side with more room), clamped inside the viewport.
 */
export function placeMenu(anchor, size, viewport, { gap = 4, margin = 8 } = {}) {
  const below = viewport.height - anchor.bottom - gap - margin;
  const above = anchor.top - gap - margin;
  const up = size.height > below && above > below;
  const room = Math.max(up ? above : below, 0);
  const height = Math.min(size.height, room);
  const top = up ? anchor.top - gap - height : anchor.bottom + gap;
  const left = Math.min(Math.max(margin, anchor.right - size.width), Math.max(margin, viewport.width - margin - size.width));
  return { left, top, maxHeight: Math.max(room, 0), up };
}
