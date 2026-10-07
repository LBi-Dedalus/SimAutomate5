// Session lifecycle: talks to the backend on behalf of the store. DOM-free; the Tauri
// `invoke` and the storage are injected so the exact logic is covered by tests.
//
// Rules that every method follows:
// - the session id (and attempt) is captured BEFORE any await, never re-read from "active";
// - a session is registered in the store before its first connect, so early events are kept;
// - the backend result never regresses a newer state observed through events.
import { createSessionStore, isBusyStatus, isTerminalStatus, takeLegacyDraft } from "./sessions-core.js";

export function createSessionController({ invoke, storage, log = {}, store = createSessionStore() }) {
  const logError = log.error ?? (() => {});
  const logWarn = log.warn ?? (() => {});
  /** Per-session chain: connect / disconnect / close never overlap for one session. */
  const chains = new Map();
  let legacyChecked = false;

  function serial(id, task) {
    const previous = chains.get(id) ?? Promise.resolve();
    const run = previous.then(task, task);
    chains.set(
      id,
      run.catch(() => {}),
    );
    return run;
  }

  function fail(id, text, where, err) {
    logError(`${text}: ${String(err)}`, where);
    store.addLocal(id, "systemerror", `${text}: ${String(err)}`);
  }

  function firstSessionDraft() {
    if (legacyChecked) return "";
    legacyChecked = true;
    const { draft, error } = takeLegacyDraft(storage);
    if (error) logWarn(error, "sessions:legacyDraft");
    return draft;
  }

  async function connectAttempt(id) {
    const attempt = store.beginAttempt(id);
    if (attempt === null) return { ok: false, error: "The session was closed" };
    const session = store.get(id);
    try {
      await invoke("connect_socket", { sessionId: id, attempt, req: session.req });
      store.markRegistered(id);
      return { ok: true };
    } catch (err) {
      // Only the attempt that failed is marked: a newer one keeps its own state.
      store.setStatus(id, attempt, "error");
      fail(id, "Failed to connect", "sessions:connect", err);
      return { ok: false, error: String(err) };
    }
  }

  return {
    store,

    /**
     * Creates, selects and connects a new session next to the existing ones.
     * `done` settles when the backend answered the connect command.
     */
    start({ mode, label, endpoint = null, req }) {
      const session = store.create({ mode, label, endpoint, req, draft: firstSessionDraft() });
      store.select(session.id);
      const done = serial(session.id, () => connectAttempt(session.id));
      return { id: session.id, done };
    },

    reconnect(id) {
      const session = store.get(id);
      if (!session) return Promise.resolve({ ok: false, error: "The session was closed" });
      if (!isTerminalStatus(session.status) || session.op) {
        return Promise.resolve({ ok: false, error: "The session is not disconnected" });
      }
      return serial(id, () => {
        const current = store.get(id);
        if (!current) return { ok: false, error: "The session was closed" };
        if (!isTerminalStatus(current.status)) return { ok: false, error: "The session is busy" };
        return connectAttempt(id);
      });
    },

    /** Stops the connection of one session; it stays open and can be reconnected. */
    disconnect(id) {
      if (!store.get(id)) return Promise.resolve({ ok: false, error: "The session was closed" });
      // Sends are refused from this very moment, not from when the queued task starts.
      const early = store.get(id);
      // This also covers a connect/reconnect still queued or pending (not registered yet, or
      // terminal until it starts): the queued task clears the intent when nothing was to stop.
      if (!early.op) store.setOp(id, "disconnecting");
      return serial(id, async () => {
        const session = store.get(id);
        if (!session) return { ok: false, error: "The session was closed" };
        if (!session.registered || isTerminalStatus(session.status)) {
          if (session.op === "disconnecting") store.setOp(id, null);
          return { ok: true };
        }
        const attempt = session.attempt;
        store.setOp(id, "disconnecting");
        try {
          await invoke("disconnect_socket", { sessionId: id });
          // The task is over: if its terminal event is still in flight, settle now.
          const current = store.get(id);
          if (current && !isTerminalStatus(current.status)) {
            store.setStatus(id, attempt, "disconnected");
          }
          return { ok: true };
        } catch (err) {
          fail(id, "Failed to disconnect", "sessions:disconnect", err);
          return { ok: false, error: String(err) };
        } finally {
          store.setOp(id, null);
        }
      });
    },

    /** What closing would lose, or null when the session does not exist. */
    closeImpact(id) {
      const session = store.get(id);
      if (!session) return null;
      const running = isBusyStatus(session.status);
      const messages = session.records.length;
      const draft = session.draft.trim() !== "";
      return { label: session.label, running, messages, draft, needsConfirm: running || messages > 0 || draft };
    },

    /** Stops the session and forgets it. On failure the session stays, with a visible error. */
    close(id) {
      if (!store.get(id)) return Promise.resolve({ ok: false, error: "The session was closed" });
      if (!store.get(id).op) store.setOp(id, "closing");
      return serial(id, async () => {
        const session = store.get(id);
        if (!session) return { ok: true };
        store.setOp(id, "closing");
        if (session.registered) {
          try {
            await invoke("close_session", { sessionId: id });
          } catch (err) {
            store.setOp(id, null);
            fail(id, "Failed to close the session", "sessions:close", err);
            return { ok: false, error: String(err) };
          }
        }
        // From here on, late events for this id are dropped as "unknown".
        store.remove(id);
        chains.delete(id);
        return { ok: true };
      });
    },

    /** Snapshot of a session as a send target, or null unless it can send right now. */
    sendTarget(id = store.activeId) {
      const session = id ? store.get(id) : null;
      if (!session || session.status !== "connected" || session.op) return null;
      return { id: session.id, attempt: session.attempt, label: session.label };
    },

    /**
     * Sends to the captured target. Never falls back to another session: if the target
     * was closed, changed attempt or disconnected, it is reported on that session.
     */
    async send(target, message) {
      if (!target) return { ok: false, error: "No connected session" };
      const session = store.get(target.id);
      if (!session) return { ok: false, error: "The target session was closed" };
      if (session.attempt !== target.attempt || session.status !== "connected" || session.op) {
        const error = "The session is not connected";
        store.addLocal(target.id, "systemerror", `Failed to send: ${error}`);
        return { ok: false, error };
      }
      try {
        await invoke("send_message", {
          sessionId: target.id,
          attempt: target.attempt,
          payload: { message },
        });
        return { ok: true };
      } catch (err) {
        fail(target.id, "Failed to send", "sessions:send", err);
        return { ok: false, error: String(err) };
      }
    },

    handleStatus(payload) {
      return store.applyStatus(payload);
    },

    handleMessage(payload) {
      return store.applyMessage(payload);
    },
  };
}
