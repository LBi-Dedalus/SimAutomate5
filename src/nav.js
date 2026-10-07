// Left navigation between the main views. A view can register a leave guard so
// unsaved work is never lost silently (see templates.js).

const guards = new Map();
let current = "home";

/**
 * guard(proceed) returns true when leaving is allowed right away. Otherwise it must
 * keep the user on the view, ask what to do and call proceed() once leaving is OK.
 */
export function setLeaveGuard(view, guard) {
  guards.set(view, guard);
}

export function currentView() {
  return current;
}

/**
 * Shows a view; returns false when a leave guard kept the user on the current view.
 * `onActivate` runs once the view is really shown, also when a guard deferred the
 * navigation and the user later let it proceed (and never if the user cancelled).
 */
export function showView(name, { force = false, onActivate } = {}) {
  if (name === current) {
    onActivate?.();
    return true;
  }
  const run = () => {
    activate(name);
    onActivate?.();
  };
  const guard = guards.get(current);
  if (!force && guard && !guard(run)) return false;
  run();
  return true;
}

function activate(name) {
  current = name;
  for (const panel of document.querySelectorAll("[data-view-panel]")) {
    panel.classList.toggle("hidden", panel.dataset.viewPanel !== name);
  }
  for (const item of document.querySelectorAll("#nav [data-view]")) {
    item.classList.toggle("on", item.dataset.view === name);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  for (const item of document.querySelectorAll("[data-view], [data-nav]")) {
    item.addEventListener("click", () =>
      showView(item.dataset.view ?? item.dataset.nav),
    );
  }
});
