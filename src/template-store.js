// Saved template library as last confirmed by the backend, shared with the Rules view.
//
// templates.js publishes the authoritative list after every SUCCESSFUL load / save / delete
// (and clears it when a load fails). Nothing here persists anything and drafts are never
// published: the Rules view only ever offers templates that exist in config.json.

let saved = null;
const subscribers = new Set();

/** Saved templates, or null while unknown (not loaded yet or the load failed). */
export function getSavedTemplates() {
  return saved;
}

export function publishSavedTemplates(templates) {
  saved = templates.map((t) => ({
    id: t.id,
    name: t.name,
    description: t.description ?? "",
    payload: t.payload,
    variables: (t.variables ?? []).map((v) => ({ name: v.name, default: v.default })),
  }));
  for (const fn of [...subscribers]) fn(saved);
}

export function clearSavedTemplates() {
  saved = null;
  for (const fn of [...subscribers]) fn(saved);
}

/** fn(templates|null) is called after every change. Returns an unsubscribe function. */
export function subscribeSavedTemplates(fn) {
  subscribers.add(fn);
  return () => subscribers.delete(fn);
}
