# SPECIFICATION.md

## Project Overview

This application is a Tauri-based desktop app with a Vanilla JS frontend and a Rust backend. It exchanges ASTM and HL7-style messages over TCP and supports both client connections to a remote host and a local server mode for testing.

## Functional Requirements

### UI and Theme

- The UI is built with semantic HTML, Oat CSS classes, and project-specific styling (light content area, dark sidebar, teal accents).
- A labelled left navigation switches between Home, Session, Templates, Auto reply and Autobuild (a dedicated view, not a dialog). The sidebar footer shows the live connection status badge.
- Home offers quick connect (Client/Server, host/port; there is no protocol selector), recent endpoints and a template shortcut list.
- Session shows the conversation of the selected session (bubbles plus system lines), its composer with control-character buttons, and an always-docked message inspector (Parsed/Raw/Hex of the selected message).
- Multiple sessions can be open at once (client and server, any mix). The sidebar has a **Sessions** list (selected row, endpoint, mode, status, unread badge for background traffic, a close button, and a `＋` button that goes to Home) next to the **Recent servers** shortcuts.
- Sessions are runtime-only: they are not saved, restored or reconnected when the app starts. Each session owns its endpoint, status, conversation (max 2000 messages), sent/received counters, composer draft and inspected message. Switching sessions never reconnects or mixes these.
- Home always configures a NEW connection: starting from the form or a recent endpoint opens another session, whatever the others are doing. The only guard is against a double submit of the same click. Port must be 1–65535.
- The header acts on the selected session only: **Disconnect/Stop server** keeps the session (history stays, **Reconnect** starts a new attempt), **Close** stops it and removes it. Closing a running session or one with history or a draft asks for confirmation. After the last close the empty state is shown.
- Send, control-character buttons and "Send now" from a template target the session selected at the time of the click and are disabled unless it is connected. Builder output and "Load in composer" go to the draft of the session captured when they started; if it was closed meanwhile an error is shown instead of redirecting.
- Session navigation honours the template-editor unsaved-changes guard, including a deferred "proceed".

### Persistence

- Existing form preferences (`host`, `port`, `server-port`, `input`, `output`, `no-etb`) are persisted in browser storage under `simautomate:config`. Only fields marked `data-persist` are stored. The former Auto reply fields (`autoresponse-enabled`, `astm_ack`, `hl7_type`, `hl7_code`) are no longer stored or shown; they are only read once to offer a migration into Rules (see Automatic Responses). The composer text (`message`) is no longer persisted: a previously saved value is migrated once as the draft of the first session and then removed from storage.
- Recent endpoints are stored in browser storage under `simautomate:recent-endpoints` (max 8, deduplicated by mode + lower-cased host + port; server endpoints have no host). An endpoint is recorded only after the connection was actually established (client `connected`, server `listening`). Corrupt stored data is ignored and reported in the UI.
- Recent autobuilds are stored in browser storage under `simautomate:recent-autobuilds` (max 20, newest first, deduplicated by input + No ETB, where a rebuild moves the entry to the top and refreshes its output). Each entry holds `id`, `input`, `output`, `noEtb`, `kind` (`ASTM`, `HL7` or `Raw`, derived from the input like the backend) and `lastUsed`. Only successful builds with a non-empty input are recorded. If the storage is full the oldest entries are dropped until it fits. Corrupt stored data is ignored and reported non-blockingly in the Autobuild view.
- Templates are stored in `config.json` in the Tauri app config directory (`app_config_dir()`), under the root key `templates`: an array of `{ id, name, description, payload, variables: [{ name, default }] }`. Other root keys are preserved on save.
  - Built-in templates (HL7 QRY^A19, HL7 ACK^O21, ASTM ENQ, ASTM EOT) are offered only when the file or the `templates` key is absent. They are not written until the user saves, and an empty list stays empty.
  - A malformed config file or malformed template list is reported as an error and is never overwritten.
  - Writes are atomic (temp file in the same directory, then rename); the directory is created when needed.
- Templates use `{{NAME}}` variables, distinct from `<CR>`-style control tokens. `{{NOW}}` (yyyyMMddHHmmss) and `{{CONTROL_ID}}` are resolved once when the template is loaded in the composer or sent; other variables use the values entered in the editor. Unresolved variables block loading and sending. A real line break in a payload outside an HL7/MLLP frame is sent as separate writes (`enqueue_message` splits it); an HL7/MLLP frame (`<VT>` … `<FS>` plus a following `<CR>`) is always one write, see Messages.
- Unsaved template edits are protected: leaving the editor, selecting another template, or creating a new one asks to save or discard.

### Logging

- The application writes logs to dedicated files in the app log directory:
  - `backend.log` for backend operations and errors.
  - `frontend.log` for frontend operations and errors.
- Each log line contains a timestamp, location, level, and message.
- Levels are `INF`, `WRN`, and `ERR`.
- Logging for message traffic is metadata-only and must not write raw protocol payload bodies.
- Log files use size-based rotation: 5 MB per file, keeping up to 5 rotated files per stream.

### Configuration

- Client mode accepts a host and port.
- Server mode accepts a port and binds a listener on `0.0.0.0`.
- The app exposes a simple mode selector for switching between client and server configuration.
- Configuration fields of Home are never locked by other sessions' state.
- The connection badge reflects one of five states for the selected session: disconnected, connecting, listening (server bound, waiting for a client), connected, or error.

### Connection

- Commands (all carry a frontend-generated `sessionId`; connect and send also carry a monotonically increasing `attempt`): `connect_socket(sessionId, attempt, req)`, `disconnect_socket(sessionId)`, `close_session(sessionId)`, `send_message(sessionId, attempt, payload)`. Ids are 1–64 characters of letters, digits, `-`, `_`. Invalid or unknown ids, a stale attempt, a bind conflict or a send while the session is not `connected` return an error that the UI shows in that session.
- Events `connection://status` and `message://stream` carry `session_id` and `attempt`. The frontend registers the session before invoking connect and ignores events of unknown sessions or of an older attempt, so a late event cannot change a reconnected session.
- The backend keeps a registry of sessions, each with its own transport task, message queue, ACK state and shutdown signal. Disconnect/close signal the task and wait for it (bounded by 5 s, then abort) outside the global state lock. Terminal status is published before the event, so a reconnect is accepted as soon as the frontend sees it. Panics of a transport task become an `error` status.
- Client connect attempts use a 1 second timeout.
- When a client connection times out, the backend retries until the connection succeeds or the attempt is interrupted.
- A user-triggered disconnect stops the selected session's connection cleanly and updates its status to disconnected.
- Server mode starts a TCP listener on all interfaces, emits `listening` once bound, and emits `connected` once when a client is accepted (the listener is then released).
- A failed, interrupted or closed session releases only its own transport and port.
- On app exit every transport is signalled to stop.
- Each session owns the read/write loop until disconnect, EOF, or an error occurs.

### Messaging UI

- Message display area (per session):
  - Received and sent messages are shown as conversation bubbles (received on the left, sent on the right).
  - System info, warning, and error messages may also appear in the stream; failures of commands are shown in the session they concern.
  - Each message includes a timestamp.
  - A Clear button clears the history of the selected session only.
- Input area:
  - Textarea for entering messages (one draft per session).
  - Send button sends the full message and is disabled unless the selected session is connected.
  - The input content remains in the textarea after sending.
  - Clear button resets the draft of the selected session.
  - The message area is updated from the `message://stream` event, routed by `session_id`.
  - Outgoing messages are prepared by the backend message queue, which translates control characters to human-readable tokens for display.

### Automatic Responses

Auto reply is configured entirely in ordered **Rules** (Auto reply tab). There is no default acknowledgement and no implicit ACK/NAK/EOT behaviour.

- Scope: rules and the master **Enabled** switch are GLOBAL (all open sessions and sessions started later, labelled so in the view). Saved changes reach running sessions without reconnecting. Disabling or replacing the rules drops pending delayed replies.
- Rule: `{ id, name, enabled, trigger, condition?, action, delay_ms }`.
  - Trigger: HL7 message type pattern (`QRY^A19`, `ORU^R01`, or a `*` glob such as `ORU^*` or `*`), ASTM frame (STX…ETX/ETB), or ASTM ENQ.
  - Type semantics: the incoming MSH-9 is canonicalised to its first two components, so `ORU^R01^ORU_R01` matches `ORU^R01`. `*` is the only wildcard, fully anchored and case-sensitive. A pattern containing `*` never matches an incoming HL7 `ACK` message (to avoid acknowledgement loops) unless the pattern itself starts with `ACK`; an exact type such as `ACK^A01` is deliberate and does match.
  - Condition (HL7 only, optional): first segment named `SEG`, field `n`, operator exact or `*` glob, compared with the raw field text (components included, using the message's own field/component separators; MSH-1 is the separator and MSH-2 the encoding characters). No condition means any (otherwise). A missing segment/field never matches.
  - Action: `template` (id of a saved template), `literal` (text, control tokens such as `<ACK>` allowed, at most 4096 bytes) `hl7_ack` (generated acknowledgement with a message type and a two-letter MSA code stored in the rule) or `none` ("No auto reply": stored as `{"type":"none"}` with `delay_ms` 0; no template, text, acknowledgement or delay is needed or checked).
  - Delay 0 to 60000 ms: the reply is not sent before this delay; the transport's existing pacing between frames may add latency.
- Evaluation: the first enabled matching rule wins, in list order. Disabled rules are skipped. No match sends nothing. A matching enabled `none` rule sends nothing as well (logged as informational, not an error) and stops evaluation, so lower rules never reply to that message; a disabled `none` rule is skipped like any other. If the winning rule cannot produce its reply (missing template, empty or malformed request control id…) a scoped error is logged and nothing is sent: lower rules are not tried.
- Template replies: the saved template library is reused (same `{{NAME}}` grammar, single pass, malformed or unresolved placeholders are errors, values containing line breaks are rejected). `{{NOW}}` is local `yyyyMMddHHmmss`, `{{CONTROL_ID}}` is unique per reply, `{{REQ_CONTROL_ID}}` is the MSH-10 of the request that matched and overrides any default (HL7 rules only; an empty MSH-10 is a visible failure); other variables use the saved defaults. The MSH-10 is taken from the received bytes and echoed byte for byte (a non UTF-8 id such as Latin-1 `ID-é` is not altered), in templates and generated acknowledgements alike. Request-derived text is inserted as raw bytes and never interpreted as control tokens, and values containing control bytes are refused, so a peer cannot inject extra frames. Authored template text goes through the usual control-token translation.
- Frames: the frontend still receives raw TCP chunks, but rules are evaluated only on complete messages, extracted by a per-connection buffer: MLLP `VT … FS CR`, ASTM `STX … ETX/ETB` + 2 checksum characters + `CR LF` (the checksum value is not verified), and a standalone ENQ. Split and coalesced reads are handled; NAK/EOT and stray bytes are never triggers. A standalone ACK (alone or coalesced with other data, never inside a frame) is surfaced by the same parser and releases the session's own wait for an ACK, whether or not the rules are enabled, never triggering a rule. The buffer is limited to 1 MiB; a new VT/STX resynchronises; malformed or oversized frames are reported (metadata only) and never answered.
- Delays and queue: replies are scheduled per connection (no sleeping in the receive path or under a lock); a due automatic reply is not held behind a later one or behind user messages. While the session waits for the ACK of its own ASTM frame, only a pure `<ACK>`/`<NAK>` automatic reply is released early (so two simulators answering each other cannot deadlock); every other automatic frame and user message still waits. Pending replies are cancelled on disconnect, close, reconnect, rule replacement and when disabled.
- Persistence: `config.json`, root key `auto_reply` = `{ enabled, rules: [...] }`, written under the same lock and atomic write as templates, preserving other root keys; a missing key means disabled with no rules. Rules and template references are validated before every save and at startup; templates cannot be saved or deleted if that would break a rule (the refusal names the rule). A corrupt `auto_reply` or `templates` value disables automatic replies, is reported in the Rules view and is never overwritten.
- UI: rules table (On, When receiving, Condition, Reply with, Delay) with docked editor, ordering arrows, two-step delete, per-field validation, read-only template preview, and an unsaved-changes guard (Save / Discard / Cancel). The master switch persists only the enabled flag, never a half-edited draft; while a switch change is pending every rules save (Save and the guard's Save) and Discard are refused so a save cannot write back the old flag, and the draft is kept; the sidebar badge shows the applied state.
- Migration: when `auto_reply` is absent, the previous browser-storage fields (`autoresponse-enabled`, `astm_ack`, `hl7_type`, `hl7_code`) are shown as an explicit, unsaved draft ("Imported previous settings — review and Save"): the ASTM text becomes literal rules for ENQ and frames, the HL7 type/code a per-rule generated acknowledgement for `*`. Automatic replies stay off until the user saves and enables them; the old keys are removed only after a successful save.

## Message Builder

### Overview

The application includes an Autobuild helper for constructing ASTM and MLLP messages from plain text input. It is a dedicated view (sidebar entry "Autobuild"; the composer's Autobuild button navigates to it), laid out like Templates: the header has a **Clear recent** action, the left column lists the recent autobuilds and the right column is the always-docked builder. Below 960 px the list stacks above the builder and the view scrolls.

### Recent Autobuilds

- Every successful **Build** or **Build and copy to input** records an entry (see Persistence for the format). Failed builds and empty inputs are not recorded.
- Each card shows the kind badge, a short relative time, a `No ETB` marker when set, a two-line preview of the input, a `×` delete button and a **Use in session** action. Card text is rendered as plain text.
- Clicking a card loads its input, output and No ETB into the builder (it never builds or sends); the card matching the builder is highlighted.
- **Use in session** copies the stored output (no rebuild) to the composer draft of the session active at click time and shows the Session view (honouring the template-editor unsaved-changes guard). With no session, or a closed one, an error is shown and nothing is copied.
- **Build and copy to input** delivers to the session that was active when the build started, then shows that session.
- An empty list shows an explanatory empty state.

### Features

- The helper accepts multiline text input.
- "Autobuild" sends the current content to the backend for protocol-aware building.
- Special/control characters are shown as human-readable tokens such as `<STX>`, `<CR>`, and `<VT>`.
- The built output can be copied back into the message input.
- A `No ETB?` option changes ASTM segment termination so the final control character is `ETX` for every segment instead of using `ETB` for intermediate segments.

### Build Logic

- Autobuild behavior (backend detection):
  - If input starts with `H|`, treat it as ASTM and build ASTM output.
  - If input starts with `MSH|`, treat it as HL7 and build MLLP-wrapped output.
  - Otherwise, leave the input unchanged.
- ASTM build rules:
  - Output begins with `<ENQ>` and ends with `<EOT>`.
  - Each line becomes a numbered ASTM segment.
  - Segments include checksum calculation and the appropriate `ETB` or `ETX` control character.
  - Output is rendered in human-readable token form.
- MLLP build rules (used for HL7):
  - `<VT>` is added at the beginning.
  - Each line ends with `<CR>`.
  - `<FS><CR>` is appended after the last line.

### Backend Behavior

- TCP transport and message queue handling live in the Rust backend.
- The message queue releases user messages one line at a time.
- The queue pauses between ASTM segments until an ACK is received when required.
- Received messages are emitted to the frontend as message stream events.
- Automatic responses are rule-driven (see Automatic Responses): they are scheduled per connection and sent through the same transport path as user messages.

### Commands and Events

- Frontend to backend commands:
  - `connect_socket`
  - `disconnect_socket`
  - `send_message`
  - `auto_build_message_cmd`
  - `log_frontend`
  - `load_templates`
  - `save_templates` (refused when it would break an auto reply rule; refreshes the running rules)
  - `load_auto_reply`
  - `save_auto_reply`
  - `set_auto_reply_enabled` (persists only the master switch)
- Backend to frontend events:
  - `connection://status`
  - `message://stream`

## Message Format and Protocol Support

- Messages are plain text and may contain multiple lines.
- Sending: text is split into queue items, each written to the socket separately (paced by the transport). When the text, after leading whitespace, starts with `<VT>` (or byte 0x0B), the whole frame up to `<FS>` plus a following `<CR>` is ONE item (one write, one log entry) and its composer line breaks (LF/CRLF) are not transmitted; several frames give one item each; an unterminated frame is sent as one item up to the end of the text. Any other text keeps one item per line (newlines dropped), so ASTM `<ENQ>`/`<STX>` items still wait for the peer's ACK.
- The app must support special characters such as `<ENQ>`, `<EOT>`, `<VT>`, `<FS>`, `<STX>`, `<ETX>`, `<ETB>`, `<CR>`, `<LF>`, `<ACK>`, and `<NAK>`.
- The backend translates between human-readable token strings and their corresponding control characters for sending and display.
- Message display uses human-readable token strings rather than raw binary control bytes.
