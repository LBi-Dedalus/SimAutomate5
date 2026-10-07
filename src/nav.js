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

/** Shows a view; returns false when a leave guard kept the user on the current view. */
export function showView(name, { force = false } = {}) {
  if (name === current) return true;
  const guard = guards.get(current);
  if (!force && guard && !guard(() => activate(name))) return false;
  activate(name);
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
