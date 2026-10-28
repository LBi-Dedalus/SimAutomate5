/**
 * Builds a message and delivers it to the composer draft of the session that was selected
 * when the build STARTED. A session switch during the await never redirects the output.
 *
 * deps: { invoke, activeId(), hasSession(id), setDraft(id, text) -> boolean }
 * Returns { output, target, delivered, error }.
 */
export async function buildForTarget(deps, input, noEtb, { deliver }) {
  const target = deps.activeId();
  const { output } = await deps.invoke("auto_build_message_cmd", {
    req: { input, no_etb: noEtb },
  });
  if (!deliver) return { output, target, delivered: false, error: null };
  return { output, target, ...deliverToTarget(deps, output, target) };
}

/**
 * Copies already built text to the composer draft of `target` (the session captured by the
 * caller, defaults to the active one right now). No build happens.
 * Returns { delivered, target, error }.
 */
export function deliverToTarget(deps, output, target = deps.activeId()) {
  if (!target) {
    return {
      target,
      delivered: false,
      error: "No session is open: the message was built but not copied. Start a session first.",
    };
  }
  if (!deps.hasSession(target) || !deps.setDraft(target, output)) {
    return {
      target,
      delivered: false,
      error: "The session this message was built for has been closed: nothing was copied.",
    };
  }
  return { target, delivered: true, error: null };
}
/**
 * The composer's Autobuild button: builds the text of the composer (No ETB always off, whatever
 * the Autobuild view has persisted) and replaces the draft of the session that was selected at
 * click time. Nothing is sent and the view does not change.
 *
 * deps: { invoke, activeId(), hasSession(id), setDraft(id, text) -> boolean,
 *         readComposer() -> text shown in the composer, getDraft(id) -> text,
 *         reportError(target, message) }
 * `run()` resolves true when the draft was replaced. One build per session at a time; the output
 * is dropped (and reported) when the draft was edited while the build ran.
 */
export function createComposerAutobuild(deps) {
  const pending = new Set();
  const fail = (target, message) => {
    deps.reportError(target, message);
    return false;
  };

  async function run() {
    const target = deps.activeId();
    if (!target || pending.has(target)) return false;
    const input = deps.readComposer();
    if (input.trim() === "") return fail(target, "Cannot autobuild: the message is empty.");
    // The composer is the source of truth: make the draft match it so edits can be detected.
    deps.setDraft(target, input);
    pending.add(target);
    try {
      const { output } = await buildForTarget(deps, input, false, { deliver: false });
      if (!deps.hasSession(target)) return false;
      if (deps.getDraft(target) !== input) {
        return fail(target, "Autobuild result discarded: the message was edited while it was being built.");
      }
      const delivered = deliverToTarget(deps, output, target);
      return delivered.delivered ? true : fail(target, delivered.error);
    } catch (err) {
      return fail(target, `Autobuild failed: ${err?.message ?? err}`);
    } finally {
      pending.delete(target);
    }
  }

  return { run, isPending: (id) => pending.has(id) };
}

/**
 * Applies a template's Autobuild option to an already resolved message.
 * Option off: returns the text unchanged and never calls the backend.
 * Option on: returns the built output; a backend failure rejects (nothing may be delivered).
 *
 * deps: { invoke }, options: { autobuild, noEtb }
 */
export async function applyTemplateAutobuild(deps, text, { autobuild, noEtb }) {
  if (!autobuild) return text;
  const { output } = await deps.invoke("auto_build_message_cmd", {
    req: { input: text, no_etb: Boolean(noEtb) },
  });
  return output;
}
