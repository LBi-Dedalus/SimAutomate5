# TODO.md

## Application Todo List

### UI Implementation
- [x] Create configuration section for IP and port input
- [x] Enable Connect button only when both fields are filled
- [x] Add connection retries toggle (enabled by default) on the left side of config bar
- [x] Move protocol select (ASTM / HL7 only) to the right side of config bar
- [x] Add connection status indicator (dot + label: Disconnected / Connecting / Connected) next to Connect button
- [x] Update Connect button label to "Disconnect" when connected
- [x] Add auto-response toggle and protocol-specific config on the right side of config bar
- [x] Show ASTM response textarea only when ASTM selected and auto-response enabled
- [x] Show HL7 message-type + response-code inputs only when HL7 selected and auto-response enabled
- [x] Build message display area with color coding (red for received, green for sent)
- [x] Add timestamp to each message
- [x] Add Clear button for message display area
- [x] Position message-log Clear button as a floating control in the top-right corner
- [x] Add `Open Logs` button to open the logs folder in system file explorer
- [x] Refresh UI to a lighter, more colorful, professional token-based theme
- [x] Add input textarea for message entry
- [x] Add Send and Clear buttons for input area
- [x] Keep message textarea content after Send (clear only via Clear button)
- [x] Create Message Builder page with input textarea, output area, protocol selection, and Build button
- [x] Add toggle to enable/disable automatic responses
- [x] Add UI for configuring ASTM automatic response message
- [x] Add UI for configuring HL7 automatic response (message type and response code)
- [x] Persist configuration locally (IP, port, retries, protocol, auto-response toggles/fields, last message input) and hydrate on launch
- [x] Replace builder dual textarea/protocol select with single textarea and Autobuild button
- [x] Autobuild sends content to backend; backend detects ASTM (H|) vs HL7 (MSH|) and builds accordingly, leaving other input unchanged
- [x] Forward frontend logs to dedicated `frontend.log` via Tauri command (`INF`/`WRN`/`ERR`, timestamp, location, message)

### Backend Implementation (Tauri)
- [x] Implement TCP socket communication with 1s timeout
- [x] Implement retry logic with exponential backoff (up to 5 times)
- [x] Support sending and receiving plain text messages with special character translation
- [x] Handle ASTM and HL7 protocol requirements
- [x] Implement Tauri channels for frontend-backend communication
- [x] Implement automatic response logic for ASTM (send configured message)
- [x] Implement automatic response logic for HL7 (generate response from type/code and received message)
- [x] Allow auto-response config updates while disconnected and apply stored config on next connection
- [x] Expose autobuild command that detects protocol and returns built content
- [x] Ensure user-triggered disconnect shuts down the socket cleanly
- [x] Add dedicated `backend.log` file logging for backend operations/errors (`INF`/`WRN`/`ERR`, timestamp, location, message)
- [x] Add rotating log files (5 MB per file, keep 5) for frontend and backend logs

### Message Builder Logic
- [x] Process input line by line for message building
- [x] For MLLP: prepend <VT>, append <CR> to each line, append <FS><CR> after last <CR>
- [x] For ASTM: convert each line to ASTM segment with segment number and checksum

### General
- [x] Update SPECIFICATION.md as features are designed, implemented, or changed
- [ ] Test all features and edge cases
- [ ] Document usage and protocols supported

---
_Last updated: March 2, 2026_
