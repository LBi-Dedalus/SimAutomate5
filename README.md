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
  - Autobuild to open the protocol helper.
- Autobuild helper for:
  - ASTM text with segment numbering, checksums, and control characters.
  - HL7 text wrapped as MLLP.
  - Optional `No ETB` mode for ASTM output.
- Automatic response support:
  - ASTM: configurable response message.
  - HL7: configurable message type and response code.
  - Settings are updated through the UI and stored locally. They are global: they apply to all sessions, current and future.
- Local persistence for the current UI configuration in browser storage.
- Structured logging to `backend.log` and `frontend.log` with size-based rotation.
- Metadata-only traffic logging; raw protocol payload bodies are not written to the log files.
- Special character reference buttons for ASTM/HL7 control tokens.

## Usage

1. Launch the app.
2. Choose a mode:
   - Client: enter host and port, then connect.
   - Server: enter a port, then start the server.
3. If needed, open **Auto reply** in the sidebar and configure ASTM or HL7 response values.
4. Type a message in the Session composer (or load a template).
5. Use **Autobuild** if you want the backend to format ASTM or HL7/MLLP content.
6. Click **Send** to transmit the message.
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

`cargo test` includes real loopback-socket tests for several simultaneous sessions. Auto-reply templates are not available yet.

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
