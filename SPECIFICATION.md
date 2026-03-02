# SPECIFICATION.md

## Project Overview
This application is a Tauri-based desktop app with a Vanilla JS frontend. Its purpose is to send and receive messages with another application on the local network. Communication is direct via IP address and port; DNS resolution is not required.

## Functional Requirements

### Visual Theme
- The UI uses a light, colorful, and professional theme by overriding Oat design tokens via CSS variables in `:root`.
- Styling changes must remain token-driven (using Oat variables) to preserve consistency across controls and states.

### Logging
- The application writes logs to dedicated files in the app log directory:
  - `backend.log` for backend operations and errors.
  - `frontend.log` for frontend operations and errors (forwarded through a Tauri command).
- Each log line contains: timestamp, location (`file:line`), level (`INF`, `WRN`, `ERR`), and message.
- Logging for message traffic is metadata-only (for example direction/protocol/length/status) and must not write raw message payload bodies.
- Log files use size-based rotation: 5 MB per file, keeping up to 5 rotated files per stream.
- The UI provides an `Open Logs` action that opens the log directory in the system file explorer.

### Configuration
- User can enter the IP address and port of the remote application.
- Both fields must be filled for the Connect button to be enabled.
- The IP and port are configurable at any time.
- Protocol selection (ASTM or HL7) is shown on the right side of the configuration bar. MLLP is a message-builder format only and is not a selectable connection protocol.
- A toggle to enable/disable connection retries is shown on the left side, enabled by default.
- Configuration (IP, port, retries toggle, protocol selection, auto-response toggle and fields, last message input) is persisted locally and restored on app launch.


### Connection
- When Connect is clicked, the application attempts to open a TCP socket stream to the provided IP and port.
- The connection attempt uses a timeout of 1 second.
- There is an option to allow connection retries (toggle in the UI).
- If retries are enabled and the connection fails, the application retries up to 5 times with exponential backoff (e.g., 1s, 2s, 4s, 8s, 16s).
- A status indicator (dot + label) is shown next to the Connect button:
  - **Disconnected** (grey): no active connection.
  - **Connecting** (amber): connection attempt in progress or retrying.
  - **Connected** (green): TCP socket is open and ready.
- The auto-response toggle is displayed on the right side of the configuration bar, separated from the connection controls.
- On user-triggered disconnect, the socket write half is shutdown to close the connection cleanly; application shutdown does not need an explicit clean disconnect.

### Messaging UI
- Message display area:
  - Received messages are shown in red.
  - Sent messages are shown in green.
  - Each message includes a timestamp.
  - A Clear button allows clearing the message display area and is displayed as a floating control in the top-right corner of the log.
- Input area:
  - Textarea for entering messages.
  - Send button to send the message (disabled while disconnected).
  - The message input content remains in the textarea after sending; it is only cleared via the Clear button.
  - Clear button to clear the input area.
  - Uses Tauri commands `connect_socket`, `disconnect_socket`, and `send_message` with events `connection://status` and `message://stream` to reflect live status and message flow.
  - Outgoing messages normalize control characters to readable tokens (e.g., "<CR>", "<VT>") before sending.

### Automatic Responses
- The application can automatically respond to incoming messages for ASTM and HL7 protocols.
- There is a toggle to enable or disable automatic responses.
- The auto-response toggle and its protocol-specific configuration are shown on the right side of the configuration bar, alongside the Protocol selector.
- Auto-response configuration updates are accepted both while disconnected and while connected; disconnected updates are stored and applied automatically on the next connection.
- The protocol-specific config is only shown when auto-response is enabled and the relevant protocol is selected:
  - **ASTM**: a textarea for the user to enter the response message to send automatically.
  - **HL7**: two inputs for message type (e.g. `ACK^O21`) and response code (e.g. `AA`). The program generates a response message based on this configuration and the received message.


## Message Builder

### Overview
The application includes a single message textarea with an Autobuild helper to construct messages for ASTM and MLLP protocols.

### Features
- Single textarea for entering message lines (also used for sending).
- "Autobuild" button sends the current content to the backend for protocol-aware building.
- Special/control characters are shown as human-readable tokens (e.g., "<STX>", "<CR>") for clarity.

### Build Logic
- Autobuild behavior (backend detection):
  - If input starts with "H|", treat as ASTM and build ASTM output.
  - If input starts with "MSH|", treat as HL7 and build MLLP-wrapped output.
  - Otherwise, leave the input unchanged.
- ASTM build rules:
  - Each line is converted to an ASTM segment with the correct segment number and checksum.
  - Output contains the constructed ASTM message.
- MLLP build rules (used for HL7):
  - "<VT>" is added at the beginning.
  - "<CR>" is added at the end of each line.
  - "<FS><CR>" is added after the last "<CR>".

- Tauri backend will handle TCP socket communication, including connection timeout and retry logic with exponential backoff.
- Frontend will use Vanilla JS for UI logic, including the retry option toggle.
- Communication between frontend and backend will use Tauri channels (event/message system) for sending and receiving messages, connection status, and errors.
- Event names used for cross-layer communication:
  - Status: `connection://status` (payload: status, attempts, optional message).
  - Message stream: `message://stream` (payload: direction, protocol, content, timestamp, auto_response).
- Frontend invokes Tauri commands: `connect_socket`, `disconnect_socket`, `send_message`, `auto_build_message_cmd`, and `update_auto_response`.
- UI must use the Oat component library for styling and layout, but only via semantic HTML elements (div, button, input, etc.) and Oat CSS classes as described in the Oat documentation. Do not use web components or custom elements for Oat.
- UI must provide configuration for automatic responses (toggle, ASTM response message, HL7 message type and response code).
- Backend must handle automatic response logic for ASTM and HL7, generating and sending responses as configured.
- All features must be implemented from scratch.


## Message Format and Protocol Support
- Messages are plain text to support ASTM and HL7 protocols, as well as other formats.
- The app must support special characters (e.g., <STX>, <EOT>, <VT>, etc.) required by ASTM and HL7.
- The app will automatically translate protocol strings such as "<VT>", "<STX>", and others to their corresponding character values when sending and displaying messages.

## Open Questions


