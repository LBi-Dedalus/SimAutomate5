// The connection-details tooltip: ONE shared popup (#conn-tooltip, role="tooltip") describing the row
// or card the pointer or keyboard focus is on. Sessions and recent connections both use bindTooltip()
// with the rows built by connectionDetails() (home/recent-core.js). The popup never takes the
// pointer or the focus (pointer-events: none), so the buttons under it keep working.
//
//   bindTooltip(row, { key, rows, describes, prefer })   after (re)building a row
//   syncTooltip()                                         once a whole list has been rebuilt
//
// A list render replaces its nodes: syncTooltip() moves a visible tooltip to the new node with the
// same key (and refreshes its rows), or hides it when that entry is gone.
import { el } from "./render.js";

export const TOOLTIP_ID = "conn-tooltip";

const OPPOSITE = { bottom: "top", top: "bottom", right: "left", left: "right" };
const clamp = (value, min, max) => Math.min(Math.max(value, min), Math.max(min, max));

/**
 * Where to put a tooltip next to an anchor, kept inside the viewport. Pure.
 * anchor: { left, top, right, bottom }; size: { width, height }; viewport: { width, height }.
 * Tries the preferred side, its opposite, then the others, and takes the first one that fits
 * entirely; the position is finally clamped to the margin so it never leaves the window.
 * Returns { left, top, side }.
 */
export function placeTooltip({ anchor, size, viewport, prefer = "bottom", gap = 8, margin = 8 }) {
  const fits = {
    bottom: anchor.bottom + gap + size.height <= viewport.height - margin,
    top: anchor.top - gap - size.height >= margin,
    right: anchor.right + gap + size.width <= viewport.width - margin,
    left: anchor.left - gap - size.width >= margin,
  };
  const side = [prefer, OPPOSITE[prefer], "bottom", "top", "right", "left"].find((s) => fits[s]) ?? prefer;
  const at = {
    bottom: { left: anchor.left, top: anchor.bottom + gap },
    top: { left: anchor.left, top: anchor.top - gap - size.height },
    right: { left: anchor.right + gap, top: anchor.top },
    left: { left: anchor.left - gap - size.width, top: anchor.top },
  }[side];
  return {
    side,
    left: clamp(at.left, margin, viewport.width - size.width - margin),
    top: clamp(at.top, margin, viewport.height - size.height - margin),
  };
}

/**
 * The tooltip controller. getElement() returns the popup node, getViewport() the window size and
 * doc receives the global listeners (Escape, scroll, resize), added when a tooltip first shows.
 */
export function createTooltipController({ getElement, getViewport, doc }) {
  let shown = null; // the entry on display
  let globals = false;
  const latest = new Map(); // key -> entry bound since the last sync()

  const active = new Set(); // entries currently hovered or focused

  function fill(tip, entry) {
    const text = entry.hint ? entry.hint() : "";
    tip.classList.remove("text");
    if (entry.hint) {
      tip.classList.add("text");
      tip.replaceChildren(el("span", "", text));
      return;
    }
    const rows = entry.rows;
    const list = el("dl");
    for (const { label, value, code } of rows) {
      list.appendChild(el("dt", "", label));
      list.appendChild(el("dd", code ? "code" : "", value));
    }
    tip.replaceChildren(list);
  }

  function place(entry) {
    const tip = getElement();
    if (!tip) return;
    const size = tip.getBoundingClientRect();
    const { left, top } = placeTooltip({
      anchor: entry.anchor.getBoundingClientRect(),
      size: { width: size.width, height: size.height },
      viewport: getViewport(),
      prefer: entry.prefer,
    });
    tip.style.left = `${left}px`;
    tip.style.top = `${top}px`;
  }

  function show(entry) {
    const tip = getElement();
    if (!tip || (entry.hint ? !entry.hint() : entry.rows.length === 0)) return hide();
    if (shown && shown !== entry) shown.describes.removeAttribute?.("aria-describedby");
    fill(tip, entry);
    place(entry);
    tip.classList.add("show");
    entry.describes.setAttribute?.("aria-describedby", TOOLTIP_ID);
    shown = entry;
    bindGlobals();
  }

  function hide() {
    if (!shown) return;
    shown.describes.removeAttribute?.("aria-describedby");
    getElement()?.classList.remove("show");
    shown = null;
  }

  const hintActive = () => [...active].some((e) => e.hint && e.anchor.isConnected !== false);

  // A row tooltip yields to the hint of an action button inside it, and returns when that ends.
  function update(entry) {
    if (entry.hover || entry.focus) {
      active.add(entry);
      if (entry.hint || !hintActive()) show(entry);
      return;
    }
    active.delete(entry);
    if (shown !== entry) return;
    hide();
    const back = [...active].reverse().find((e) => e.anchor.isConnected !== false && (e.hint || !hintActive()));
    if (back) show(back);
  }

  function bindGlobals() {
    if (globals || !doc?.addEventListener) return;
    globals = true;
    // Escape dismisses without being consumed: dialogs and other handlers still see it.
    doc.addEventListener("keydown", (event) => {
      if (event.key !== "Escape" || !shown) return;
      for (const entry of active) entry.hover = entry.focus = false;
      active.clear();
      shown.hover = shown.focus = false;
      hide();
    });
    const reposition = () => {
      if (!shown) return;
      if (shown.anchor.isConnected === false) hide();
      else place(shown);
    };
    doc.addEventListener("scroll", reposition, true);
    globalThis.window?.addEventListener?.("resize", reposition);
  }

  /** Makes `anchor` show `rows` while hovered or keyboard-focused (focus inside it counts). */
  function bind(anchor, { key, rows, describes = anchor, prefer = "bottom" }) {
    const entry = { anchor, key, rows, describes, prefer, hover: false, focus: false };
    latest.set(key, entry);
    anchor.addEventListener("mouseenter", () => {
      entry.hover = true;
      update(entry);
    });
    anchor.addEventListener("mouseleave", () => {
      entry.hover = false;
      update(entry);
    });
    anchor.addEventListener("focusin", (event) => {
      // Only keyboard focus: a mouse click must not pin the tooltip until the next blur.
      entry.focus = event.target?.matches?.(":focus-visible") ?? true;
      update(entry);
    });
    anchor.addEventListener("focusout", (event) => {
      if (event.relatedTarget && anchor.contains?.(event.relatedTarget)) return;
      entry.focus = false;
      update(entry);
    });
    return entry;
  }

  /**
   * A text-only hint for a small action button: hover/keyboard focus shows it, leaving, Escape or a
   * click hides it. `text` is a string or a function (read each time it shows, so it can change).
   */
  function bindHint(button, { key, text, prefer = "bottom" }) {
    const hint = typeof text === "function" ? text : () => text;
    const entry = bind(button, { key, rows: [], describes: {}, prefer });
    entry.hint = hint;
    button.addEventListener("click", () => {
      entry.hover = entry.focus = false;
      active.delete(entry);
      if (shown === entry) hide();
    });
    return entry;
  }

  /** After a list render: follow the visible tooltip to its rebuilt node, or drop it. */
  function sync() {
    for (const entry of active) if (entry.anchor.isConnected === false) active.delete(entry);
    if (shown) {
      const next = latest.get(shown.key);
      if (next && next !== shown) {
        // The old node is gone: its flags are stale, so ask the new node (when the browser can say).
        const live = (selector, fallback) => next.anchor.matches?.(selector) ?? fallback;
        next.hover = live(":hover", shown.hover);
        next.focus = live(":focus-within", shown.focus);
        if (next.hover || next.focus) {
          active.add(next);
          show(next);
        } else hide();
      } else if (shown.anchor.isConnected === false) {
        hide();
      }
    }
    latest.clear();
  }

  return { bind, bindHint, sync, hide, isShown: () => shown !== null };
}

let shared;
const controller = () =>
  (shared ??= createTooltipController({
    getElement: () => document.getElementById(TOOLTIP_ID),
    getViewport: () => ({ width: window.innerWidth, height: window.innerHeight }),
    doc: document,
  }));

export const bindTooltip = (anchor, options) => controller().bind(anchor, options);
export const bindHint = (button, options) => controller().bindHint(button, options);
export const syncTooltip = () => controller().sync();
