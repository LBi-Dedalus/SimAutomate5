# Copilot Instructions for SimAutomate5

## Project Overview
- This is a Tauri desktop application with a Vanilla JS frontend.
- Purpose: Send and receive ASTM/HL7 (and other) messages over TCP sockets on a local network, with message building and auto-response features.

## Architecture & Key Components
- **Frontend**: Located in `src/` (HTML, CSS, JS). Handles UI, user input, and communicates with backend via Tauri channels.
- **Backend**: Located in `src-tauri/` (Rust). Handles TCP socket connections, message parsing, protocol logic, and auto-responses.
- **Message Builder**: UI page for constructing ASTM/MLLP messages from user input.
- **Auto-Response**: Configurable per protocol (ASTM: user-defined message; HL7: generated from type/code and incoming message).

## Developer Workflows
- **Build/Run**: Use Tauri commands with Deno (`deno task tauri dev` or via VS Code Tauri extension).
- **Specification & Tasks**: Always follow the requirements and priorities in `SPECIFICATION.md` and `TODO.md`. Keep these files up-to-date as features are designed, implemented, or changed. All development should be traceable to these documents.
- **Backend**: Rust code in `src-tauri/src/`. Main entry: `main.rs`.
- **Frontend**: JS entry: `src/main.js`, HTML: `src/index.html`.
- **Message Protocols**: Special character translation (e.g., `<STX>`, `<EOT>`, `<VT>`) is required for ASTM/HL7.
- **Configuration**: IP/port, retry logic, auto-response, and protocol settings are user-configurable via UI.

- Use Tauri's event/message system for all frontend-backend communication.
- All protocol-specific logic (ASTM, HL7, MLLP) is handled in the backend.
- UI state and configuration are managed in Vanilla JS; keep logic modular and event-driven.
- Message display: Sent (green), Received (red), with timestamps.
- Retry logic: Exponential backoff (1s, 2s, 4s, 8s, 16s) for TCP connect.
- Use Deno for all JS/TS scripts and tasks instead of npm. Prefer `deno task` for running scripts and development commands.

## Integration Points
- **Tauri Channels**: All cross-component communication uses Tauri's event system.
- **Socket Communication**: Only backend opens sockets; frontend requests actions via Tauri.
- **Message Builder**: Converts user input to protocol-compliant messages (ASTM/MLLP) before sending.

## Key Files/Directories
- `src/`: Frontend code (UI, JS logic)
- `src-tauri/`: Backend (Rust, Tauri config)
- `src-tauri/src/main.rs`: Main backend logic
- `src/main.js`: Main frontend logic
- `SPECIFICATION.md`, `TODO.md`: Project requirements and task tracking

## Examples
- To add a new protocol, implement parsing and response logic in `src-tauri/src/` and expose via Tauri events.
- To add a new UI feature, update `src/index.html` and `src/main.js`, and connect to backend via Tauri channels.

---
For more details, see SPECIFICATION.md and TODO.md in the project root.
