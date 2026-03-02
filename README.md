# SimAutomate5

SimAutomate5 is a Tauri desktop app for sending and receiving ASTM/HL7-style messages over TCP on a local network, with protocol-aware message building and configurable automatic responses.

## Features

- TCP client connection with configurable IP/port and optional retry behavior.
- Live connection status indicator (Disconnected / Connecting / Connected / Error).
- Message log with timestamps and color-coded directions:
	- Sent messages in green.
	- Received messages in red.
- Message input area with:
	- Send
	- Clear
	- Autobuild (ASTM/HL7-aware helper)
- Automatic response support:
	- ASTM: configurable response message.
	- HL7: configurable message type + response code.
	- Can be configured while disconnected; settings are stored and applied on next connection.
- Structured frontend/backend file logging:
	- `frontend.log` and `backend.log`.
	- Rotating logs (5 MB per file, up to 5 files each).
	- Metadata-focused logs (no raw protocol payload bodies).
- `Open Logs` button to open the log directory in the system file explorer.

## Usage

1. Launch the app.
2. In the top configuration bar:
	 - Enter target IP and port.
	 - Choose protocol (`ASTM` or `HL7`).
	 - Enable/disable retries as needed.
3. (Optional) Enable **Auto-Response** and set protocol-specific fields:
	 - ASTM response message, or
	 - HL7 message type and response code.
4. Click **Connect**.
5. In the message area:
	 - Enter or edit a message.
	 - Use **Autobuild** to transform raw input into protocol-compliant output.
	 - Click **Send**.
6. Monitor message traffic in the log panel.
7. Use:
	 - **Clear** (log area) to clear displayed messages.
	 - **Open Logs** to inspect log files on disk.

## Development Setup

### Prerequisites

- Rust (stable) with Cargo.
- Deno (used to run Tauri CLI tasks in this project).
- Tauri platform prerequisites for your OS (WebView/runtime toolchain).

### Install dependencies

From the project root:

```bash
deno install
```

### Run in development

```bash
deno task tauri dev
```

### Build desktop app

```bash
deno task tauri build
```

### Backend compile check

```bash
cd src-tauri
cargo check
```

## Project Structure

- `src/` — Frontend (HTML/CSS/Vanilla JS).
- `src-tauri/` — Rust backend and Tauri configuration.
- `SPECIFICATION.md` — Functional requirements.
- `TODO.md` — Implementation tracking.

## Notes for Contributors

- Keep frontend-backend communication through Tauri commands/events.
- Keep protocol-specific logic in backend Rust code.
- Update `SPECIFICATION.md` and `TODO.md` when behavior changes.
