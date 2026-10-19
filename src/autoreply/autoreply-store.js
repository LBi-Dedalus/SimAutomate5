// Effective auto reply configuration as last confirmed by the backend, shared between modules.
//
// The backend is the single source of truth. Commands that change it indirectly (a template
// save returns the effective config, default rules included) publish the result here so the
// Rules view and the sidebar badge follow it. Nothing here persists anything.

let confirmed = null;
let confirmedRevision = -1;
const subscribers = new Set();

/** Last published { revision, enabled, rules }, or null when nothing was published yet. */
export function getConfirmedAutoReply() {
  return confirmed;
}

/**
 * config = { revision, enabled, rules }. The backend revision grows with every save but IPC
 * responses can be delivered out of order: a publication that is not newer is ignored.
 */
export function publishAutoReply(config) {
  if (config.revision <= confirmedRevision) return;
  confirmedRevision = config.revision;
  confirmed = {
    revision: config.revision,
    enabled: Boolean(config.enabled),
    rules: structuredClone(config.rules ?? []),
  };
  for (const fn of [...subscribers]) fn(confirmed);
}

/** fn({ revision, enabled, rules }) is called after every publication. Returns an unsubscribe function. */
export function subscribeAutoReply(fn) {
  subscribers.add(fn);
  return () => subscribers.delete(fn);
}
