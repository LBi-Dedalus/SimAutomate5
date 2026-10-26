// Groups the ASTM units (ENQ, frames, ACK/NAK, EOT) reported by the backend into one
// message per transfer. Pure module (no DOM, no Tauri).
//
// A transfer is ENQ -> frames (each answered by ACK / NAK) -> EOT. It is one record of the
// session: `record.transfer` holds the entries (what the transmitter sent) and, on each entry,
// the reply of the other side. `record.content` / `record.raw` are the transmitter's items
// only (one per line, ready to be sent again), the replies are hints.

export const ASTM_IDLE_TIMEOUT_MS = 30_000;

const OTHER = { sent: "received", received: "sent" };

export function createAstmState() {
  return { open: { sent: null, received: null } };
}

/** True for the payloads the grouping handles. */
export function isAstmUnit(payload) {
  return (
    (payload?.msg_type === "sent" || payload?.msg_type === "received") &&
    typeof payload.astm?.kind === "string"
  );
}

function timeOf(payload) {
  const ms = Date.parse(payload.timestamp);
  return Number.isNaN(ms) ? Date.now() : ms;
}

function bytesOf(payload) {
  return Array.isArray(payload.raw) ? payload.raw : [];
}

function entryOf(payload, kind) {
  return {
    kind,
    timestamp: payload.timestamp,
    content: payload.content,
    raw: bytesOf(payload),
    frame: kind === "frame" ? payload.astm : null,
    reply: null,
  };
}

function replyOf(payload) {
  return { kind: payload.astm.kind, timestamp: payload.timestamp, content: payload.content };
}

/** Recomputes what is derived from the entries. */
function refresh(record) {
  const { transfer } = record;
  record.content = transfer.entries.map((entry) => entry.content).join("\n");
  record.raw = transfer.entries.flatMap((entry) => entry.raw);
  transfer.hasErrors = transfer.entries.some(
    (entry) => entry.frame !== null && !entry.frame.checksum_ok,
  );
}

function close(state, record, status, updated) {
  const { transfer } = record;
  if (transfer.status !== "open") return;
  transfer.status = status;
  state.open[transfer.direction] = null;
  updated.add(record);
}

function lastEntry(record) {
  return record.transfer.entries.at(-1) ?? null;
}

function endedByEtx(record) {
  const last = lastEntry(record);
  return last?.frame?.terminator === "ETX";
}

/**
 * Applies one ASTM unit. `create(fields)` appends a new record to the session and returns it.
 * Returns the records created and updated by the unit.
 */
export function applyAstmUnit(state, payload, create) {
  const updated = new Set();
  let created = null;
  const direction = payload.msg_type;
  const other = OTHER[direction];
  const at = timeOf(payload);

  closeStale(state, at, updated);

  const orphan = () => {
    created = create({ ...payload });
  };
  const open = (entry, implicit) => {
    const transfer = {
      direction,
      status: "open",
      implicit,
      lastAt: at,
      hasErrors: entry.frame !== null && !entry.frame.checksum_ok,
      entries: [entry],
    };
    const { astm: _unit, ...fields } = payload;
    created = create({ ...fields, transfer });
    state.open[direction] = created;
  };
  const touch = (record) => {
    record.transfer.lastAt = at;
    refresh(record);
    updated.add(record);
  };

  switch (payload.astm.kind) {
    case "enq":
      if (state.open[direction]) close(state, state.open[direction], "incomplete", updated);
      open(entryOf(payload, "enq"), false);
      break;

    case "frame": {
      let record = state.open[direction];
      // After the last frame (ETX) only EOT is expected: anything else starts a new message.
      if (record && endedByEtx(record)) {
        close(state, record, "incomplete", updated);
        record = null;
      }
      if (record) {
        record.transfer.entries.push(entryOf(payload, "frame"));
        touch(record);
      } else {
        open(entryOf(payload, "frame"), true);
      }
      break;
    }

    case "ack":
    case "nak": {
      const record = state.open[other];
      const last = record ? lastEntry(record) : null;
      if (last && !last.reply) {
        last.reply = replyOf(payload);
        touch(record);
      } else {
        orphan();
      }
      break;
    }

    case "eot": {
      const record = state.open[direction];
      const answered = state.open[other];
      const last = answered ? lastEntry(answered) : null;
      if (record) {
        record.transfer.entries.push(entryOf(payload, "eot"));
        touch(record);
        close(state, record, "complete", updated);
      } else if (last && !last.reply) {
        // The receiver ends the transfer instead of answering a frame.
        last.reply = replyOf(payload);
        touch(answered);
        close(state, answered, "incomplete", updated);
      } else {
        orphan();
      }
      break;
    }

    default:
      orphan();
  }
  return { created, updated: [...updated] };
}

function closeStale(state, at, updated) {
  for (const direction of ["sent", "received"]) {
    const record = state.open[direction];
    if (record && at - record.transfer.lastAt > ASTM_IDLE_TIMEOUT_MS) {
      close(state, record, "incomplete", updated);
    }
  }
}

/** Closes the open transfer of one direction as incomplete (its idle timer fired). */
export function expireAstm(state, direction) {
  const updated = new Set();
  const record = state.open[direction];
  if (record) close(state, record, "incomplete", updated);
  return [...updated];
}

/** Closes every open transfer (end of the attempt); returns the records that changed. */
export function closeAstm(state) {
  const updated = new Set();
  for (const direction of ["sent", "received"]) {
    const record = state.open[direction];
    if (record) close(state, record, "incomplete", updated);
  }
  return [...updated];
}

/** Forgets a record that left the history. */
export function forgetAstmRecord(state, record) {
  for (const direction of ["sent", "received"]) {
    if (state.open[direction] === record) state.open[direction] = null;
  }
}

/**
 * Splits the frames of a transfer: `merged` are the acknowledged ones (and the one still
 * waiting for its answer while the transfer is open); the others (NAK, no answer) are kept
 * apart, in their order.
 */
export function partitionFrames(transfer) {
  const merged = [];
  const apart = [];
  const lastIndex = transfer.entries.length - 1;
  transfer.entries.forEach((entry, index) => {
    if (entry.kind !== "frame") return;
    const waiting = !entry.reply && transfer.status === "open" && index === lastIndex;
    if (entry.reply?.kind === "ack" || waiting) merged.push(entry);
    else apart.push(entry);
  });
  return { merged, apart };
}

/** Concatenated text of frames (records may continue over an ETB frame). */
export function framesText(entries) {
  return entries.map((entry) => entry.frame.text).join("");
}

/** Counts of the answers of a transfer. */
export function replyCounts(transfer) {
  const counts = { ack: 0, nak: 0, none: 0, frames: 0 };
  for (const entry of transfer.entries) {
    if (entry.kind !== "frame") continue;
    counts.frames += 1;
    if (entry.reply?.kind === "ack") counts.ack += 1;
    else if (entry.reply) counts.nak += 1;
    else counts.none += 1;
  }
  return counts;
}
