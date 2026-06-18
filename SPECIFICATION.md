# SPECIFICATION.md

## Project Overview

This application is a Tauri-based desktop app with a Vanilla JS frontend and a Rust backend. It exchanges ASTM and HL7-style messages over TCP and supports both client connections to a remote host and a local server mode for testing.

## Functional Requirements

### UI and Theme

- The UI is built with semantic HTML, Oat CSS classes, and project-specific styling.
- The header exposes a client/server mode switch and a live connection status badge.
- The active connection mode cannot be changed while a connection attempt is in progress or while connected.

### Persistence

- The current form state is persisted in browser storage under `simautomate:config`.
- Persisted values are restored when the app loads.
- Auto-response settings are updated from the UI and survive reloads through the same persistence mechanism.

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
- Server mode accepts a port and binds a local listener on `127.0.0.1`.
- The app exposes a simple mode selector for switching between client and server configuration.
- Configuration fields are disabled while a connection is being established or while the app is connected.
- The connection badge reflects one of four states: disconnected, connecting, connected, or error.

### Connection

- Client connect attempts use a 1 second timeout.
- When a client connection times out, the backend retries until the connection succeeds or the attempt is interrupted.
- A user-triggered disconnect stops the current connection cleanly and updates the status to disconnected.
- Server mode starts a local TCP listener and waits for a client to connect.
- The active connection owns the read/write loop until disconnect, EOF, or an error occurs.

### Messaging UI

- Message display area:
  - Received messages are shown in red.
  - Sent messages are shown in green.
  - System info, warning, and error messages may also appear in the stream.
  - Each message includes a timestamp.
  - A Clear button clears the message history.
- Input area:
  - Textarea for entering messages.
  - Send button sends the full message and is disabled while disconnected.
  - The input content remains in the textarea after sending.
  - Clear button resets the input field.
  - The message area is updated from the `message://stream` event.
  - Outgoing messages are prepared by the backend message queue, which translates control characters to human-readable tokens for display.

### Automatic Responses

- The application can automatically respond to incoming ASTM and HL7 messages.
- A toggle enables or disables automatic responses.
- ASTM auto-response uses a configured response string when an incoming message begins with the ASTM `STX` control character.
- HL7 auto-response generates an ACK when an incoming message begins with the MLLP `VT` control character.
- HL7 auto-response uses the configured message type and response code, and preserves the incoming control ID in the generated ACK.
- Auto-response configuration updates are accepted while disconnected or while connected.
- The current auto-response configuration is mirrored in backend state and applied to the next connection when needed.

## Message Builder

### Overview

The application includes an Autobuild helper for constructing ASTM and MLLP messages from plain text input.

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
- Automatic responses are queued and sent through the same transport path as user messages.

### Commands and Events

- Frontend to backend commands:
  - `connect_socket`
  - `disconnect_socket`
  - `send_message`
  - `auto_build_message_cmd`
  - `update_auto_response`
  - `log_frontend`
- Backend to frontend events:
  - `connection://status`
  - `message://stream`

## Message Format and Protocol Support

- Messages are plain text and may contain multiple lines.
- The app must support special characters such as `<ENQ>`, `<EOT>`, `<VT>`, `<FS>`, `<STX>`, `<ETX>`, `<ETB>`, `<CR>`, `<LF>`, `<ACK>`, and `<NAK>`.
- The backend translates between human-readable token strings and their corresponding control characters for sending and display.
- Message display uses human-readable token strings rather than raw binary control bytes.
