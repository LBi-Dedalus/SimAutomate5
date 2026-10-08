# SimAutomate5

SimAutomate5 is a Tauri desktop app for exchanging ASTM and HL7-style messages over TCP. It includes both a client connector and a local server mode, a protocol-aware message builder, automatic response handling, and structured frontend/backend logging.

## Features

- Client mode connects to a remote host and port over TCP.
- Server mode starts a listener on `0.0.0.0` for one client connection.
- Live connection status indicator: Disconnected, Connecting, Listening (server), Connected, or Error.
- Multiple simultaneous sessions (client and/or server), listed in the sidebar; each has its own conversation, counters, draft and inspector state. Sessions are runtime-only (not restored at launch).
- Labelled left navigation: Home, Session, Templates, Auto reply, Autobuild.
- Home quick connect (Client/Server, host/port) always opens a new session; recent endpoints (browser storage) and template shortcuts.
- Session view with conversation bubbles for sent, received, and system messages with timestamps, plus a docked message inspector (Parsed HL7 / Raw / Hex of the selected message; parsing is best effort per received chunk) and "Save as template".
- Templates: list with search and an always-docked editor (create, edit, delete, `{{VARIABLES}}`, exact preview, load in composer, send). Templates are stored in `config.json` in the app config directory; built-ins are offered only when no templates have been saved yet.
- Message composer with:
  - Send for the current full message.
  - Clear to reset the input.
  - Autobuild to open the Autobuild view.
- Autobuild view (dedicated navigation entry, not a popup) with a builder (Input, Output, `No ETB ?`, **Build**, **Build and copy to input**) and a list of the 20 most recent successful autobuilds (kept in browser storage). Click a recent autobuild to load it into the builder, **Use in session** to copy its output to the active session's composer, `×` to delete it, **Clear recent** to empty the list. The builder supports:
  - ASTM text with segment numbering, checksums, and control characters.
  - HL7 text wrapped as MLLP.
  - Optional `No ETB` mode for ASTM output.
- Automatic response support (Auto reply tab, global to all sessions, current and future):
  - An ordered list of rules; each rule has a trigger (HL7 message type with an optional field condition, ASTM frame, or ASTM ENQ), a reply (saved template, literal text, or a generated HL7 acknowledgement with its own type and code, or **No auto reply**), and a delay (0 to 60000 ms). The first enabled matching rule wins; a **No auto reply** rule sends nothing and stops the lower-priority rules for that message; if none matches nothing is sent. There is no default acknowledgement.
  - Rules are stored in `config.json` (`auto_reply` key), with a global Enabled switch. Template replies use the saved template library and its `{{NAME}}` syntax; `{{REQ_CONTROL_ID}}` is filled from the MSH-10 of the request.
  - Previous browser-storage settings are offered once as an unsaved draft of explicit rules (automatic replies stay off until saved and enabled).
- Local persistence for the current UI configuration in browser storage.
- Structured logging to `backend.log` and `frontend.log` with size-based rotation.
- Metadata-only traffic logging; raw protocol payload bodies are not written to the log files.
- Special character reference buttons for ASTM/HL7 control tokens.

## Usage

1. Launch the app.
2. Choose a mode:
   - Client: enter host and port, then connect.
   - Server: enter a port, then start the server.
3. If needed, open **Auto reply**, add rules (when receiving → reply with), save them and turn **Enabled** on.
4. Type a message in the Session composer (or load a template).
5. Use **Autobuild** (sidebar or composer button) if you want the backend to format ASTM or HL7/MLLP content; pick one of the recent autobuilds to reuse it.
6. Click **Send** to transmit the message. An HL7/MLLP message (starting with `<VT>`, ending with `<FS><CR>`) is sent as a single write; the line breaks between its segments are only composer formatting and are not transmitted. Other text (e.g. ASTM) is sent line by line.
7. Use **Clear** in the Session header to clear the selected conversation, or **Clear** in the composer to reset its draft.
8. Go back to Home (or the `＋` in the sidebar) to open more sessions; switch with the Sessions list. **Disconnect** keeps a session (use **Reconnect**), **Close** removes it.

## Development Setup

### Prerequisites

- Rust stable with Cargo.
- Deno for the Tauri CLI tasks used by this project.
- The platform-specific Tauri/WebView prerequisites for your OS.

### Install dependencies

From the project root:

```bash
deno install
```

### Run in development

```bash
deno task tauri dev
```

### Build the desktop app

```bash
deno task tauri build
```

### Backend compile check

```bash
cd src-tauri
cargo check
```

### Tests

```bash
node --test tests/*.test.js   # on Windows PowerShell: $f=(Get-ChildItem tests\*.test.js).FullName; node --test @f
cd src-tauri
cargo test
```

`cargo test` includes real loopback-socket tests for several simultaneous sessions and for auto reply rules (first match, templates, split/coalesced frames, delays, reconnect, ACK-wait policy). The Node tests cover the Rules view with a fake DOM and a mocked IPC.

### Manual socket test helpers

The repository also includes `test-socket-client.js` and `test-socket-server.js` for ad hoc TCP testing.

## Project Structure

- `src/` - Frontend HTML, CSS, and Vanilla JS.
- `src-tauri/` - Rust backend and Tauri configuration.
- `SPECIFICATION.md` - Functional requirements and behavior notes.
- `test-socket-client.js` / `test-socket-server.js` - Manual socket test helpers.

## Notes for Contributors

- Keep frontend-backend communication through Tauri commands and events.
- Keep protocol-specific logic in backend Rust code unless the behavior is purely presentational.
- Update `SPECIFICATION.md` when behavior changes.
