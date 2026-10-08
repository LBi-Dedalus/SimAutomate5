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