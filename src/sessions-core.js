// Runtime-only session store. Pure module (no DOM, no Tauri): every session owns its
// status, history, composer draft and inspected message, so nothing can leak from one
// session into another. Sessions are never persisted nor restored.

export const MAX_MESSAGES = 2000;

const BUSY = ["connecting", "listening", "connected"];
export const isBusyStatus = (status) => BUSY.includes(status);
export const isTerminalStatus = (status) =>
  status === "disconnected" || status === "error";

const CONFIG_KEY = "simautomate:config";

/**
 * Before sessions existed the composer text was persisted globally under `message`.
 * It is handed over once, as the draft of the first session, then removed from the stored
 * config so it is not resurrected on every launch. Returns { draft, error }.
 */
export function takeLegacyDraft(storage) {
  try {
    const raw = storage.getItem(CONFIG_KEY);
    if (!raw) return { draft: "", error: null };
    const data = JSON.parse(raw);
    if (typeof data?.message !== "string") return { draft: "", error: null };
    const draft = data.message;
    delete data.message;
    storage.setItem(CONFIG_KEY, JSON.stringify(data));
    return { draft, error: null };
  } catch (err) {
    return { draft: "", error: `Cannot migrate the saved draft: ${String(err)}` };
  }
}

function defaultNewId(counter) {
  const random = Math.random().toString(36).slice(2, 8);
  // Must satisfy the backend rule: 1-64 characters among letters, digits, '-' and '_'.
  return `s-${Date.now().toString(36)}-${counter.toString(36)}-${random}`;
}

/**
 * @param {{ newId?: (counter: number) => string, now?: () => string, onListenerError?: (err: unknown) => void }} [options]
 */
export function createSessionStore(options = {}) {
  const newId = options.newId ?? defaultNewId;
  const now = options.now ?? (() => new Date().toISOString());
  const onListenerError = options.onListenerError ?? ((err) => console.error(err));

  /** @type {Map<string, object>} insertion ordered = sidebar order */
  const sessions = new Map();
  const listeners = new Set();
  let activeId = null;
  let counter = 0;

  function emit(type, session, extra = {}) {
    for (const listener of [...listeners]) {
      try {
        listener({ type, id: session?.id ?? extra.id ?? null, session, ...extra });
      } catch (err) {
        // A broken view must not stop the state update nor the other views.
        onListenerError(err);
      }
    }
  }

  function create({ mode, label, endpoint = null, req = null, draft = "" }) {
    counter += 1;
    const id = newId(counter);
    const session = {
      id,
      mode,
      label,
      endpoint,
      req,
      status: "connecting",
      attempt: 0,
      /** True once the backend accepted a connect for this id (so it must be closed there). */
      registered: false,
      /** Operation in flight on this session: "disconnecting" | "closing" | null. */
      op: null,
      /** Recent endpoint already recorded for this session. */
      recorded: false,
      draft,
      records: [],
      sent: 0,
      received: 0,
      nextRecordId: 0,
      selectedRecordId: null,
      unread: 0,
    };
    sessions.set(id, session);
    emit("created", session);
    return session;
  }

  const get = (id) => sessions.get(id) ?? null;

  function select(id) {
    const session = sessions.get(id);
    if (!session) return false;
    const changed = activeId !== id;
    activeId = id;
    session.unread = 0;
    emit("select", session, { changed });
    return true;
  }

  /** Starts a new attempt: older events of this session are ignored from now on. */
  function beginAttempt(id) {
    const session = sessions.get(id);
    if (!session) return null;
    session.attempt += 1;
    session.status = "connecting";
    emit("status", session);
    return session.attempt;
  }

  /** Applies a backend status event; events of unknown sessions/attempts are dropped. */
  function applyStatus(payload) {
    const session = sessions.get(payload?.session_id);
    if (!session) return { accepted: false, reason: "unknown" };
    if (payload.attempt !== session.attempt) return { accepted: false, reason: "stale" };
    session.status = payload.status;
    emit("status", session);
    return { accepted: true, session };
  }

  /** Forces a status for a given attempt (local outcome of a command). */
  function setStatus(id, attempt, status) {
    const session = sessions.get(id);
    if (!session || session.attempt !== attempt) return false;
    if (session.status === status) return true;
    session.status = status;
    emit("status", session);
    return true;
  }

  function appendRecord(session, fields) {
    const record = { id: ++session.nextRecordId, session_id: session.id, ...fields };
    session.records.push(record);
    if (record.msg_type === "sent") session.sent += 1;
    else if (record.msg_type === "received") session.received += 1;

    const evicted = [];
    while (session.records.length > MAX_MESSAGES) {
      const removed = session.records.shift();
      if (removed.msg_type === "sent") session.sent -= 1;
      else if (removed.msg_type === "received") session.received -= 1;
      if (removed.id === session.selectedRecordId) session.selectedRecordId = null;
      evicted.push(removed);
    }
    if (record.msg_type === "received" && session.id !== activeId) session.unread += 1;
    emit("message", session, { record, evicted });
    return record;
  }

  /** Applies a backend message event; unknown sessions and stale attempts are dropped. */
  function applyMessage(payload) {
    const session = sessions.get(payload?.session_id);
    if (!session) return null;
    if (payload.attempt !== session.attempt) return null;
    return appendRecord(session, { ...payload });
  }

  /** Adds a locally produced system line (rejected command, failure...) to one session. */
  function addLocal(id, msgType, content) {
    const session = sessions.get(id);
    if (!session) return null;
    return appendRecord(session, {
      attempt: session.attempt,
      msg_type: msgType,
      content,
      timestamp: now(),
      local: true,
    });
  }

  function selectRecord(id, recordId) {
    const session = sessions.get(id);
    if (!session) return null;
    const record = session.records.find((item) => item.id === recordId) ?? null;
    session.selectedRecordId = record ? record.id : null;
    emit("record-selected", session, { record });
    return record;
  }

  function setDraft(id, text) {
    const session = sessions.get(id);
    if (!session) return false;
    session.draft = text;
    emit("draft", session);
    return true;
  }

  function clearMessages(id) {
    const session = sessions.get(id);
    if (!session) return false;
    session.records = [];
    session.sent = 0;
    session.received = 0;
    session.selectedRecordId = null;
    emit("cleared", session);
    return true;
  }

  function setOp(id, op) {
    const session = sessions.get(id);
    if (!session) return false;
    session.op = op;
    emit("op", session);
    return true;
  }

  function markRegistered(id) {
    const session = sessions.get(id);
    if (session) session.registered = true;
  }

  function markRecorded(id) {
    const session = sessions.get(id);
    if (session) session.recorded = true;
  }

  /** Forgets a session. The neighbour (next, else previous) becomes selected. */
  function remove(id) {
    const session = sessions.get(id);
    if (!session) return false;
    const ids = [...sessions.keys()];
    const index = ids.indexOf(id);
    sessions.delete(id);
    emit("removed", session);
    if (activeId === id) {
      const next = ids[index + 1] ?? ids[index - 1] ?? null;
      activeId = null;
      if (next) select(next);
      else emit("select", null, { changed: true });
    }
    return true;
  }

  return {
    create,
    get,
    select,
    remove,
    beginAttempt,
    applyStatus,
    setStatus,
    applyMessage,
    addLocal,
    selectRecord,
    setDraft,
    clearMessages,
    setOp,
    markRegistered,
    markRecorded,
    list: () => [...sessions.values()],
    active: () => sessions.get(activeId) ?? null,
    get activeId() {
      return activeId;
    },
    get size() {
      return sessions.size;
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
