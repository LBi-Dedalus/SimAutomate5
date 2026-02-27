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
- [x] Add input textarea for message entry
- [x] Add Send and Clear buttons for input area
- [x] Create Message Builder page with input textarea, output area, protocol selection, and Build button
- [x] Add toggle to enable/disable automatic responses
- [x] Add UI for configuring ASTM automatic response message
- [x] Add UI for configuring HL7 automatic response (message type and response code)

### Backend Implementation (Tauri)
- [x] Implement TCP socket communication with 1s timeout
- [x] Implement retry logic with exponential backoff (up to 5 times)
- [x] Support sending and receiving plain text messages with special character translation
- [x] Handle ASTM and HL7 protocol requirements
- [x] Implement Tauri channels for frontend-backend communication
- [x] Implement automatic response logic for ASTM (send configured message)
- [x] Implement automatic response logic for HL7 (generate response from type/code and received message)

### Message Builder Logic
- [x] Process input line by line for message building
- [x] For MLLP: prepend <VT>, append <CR> to each line, append <FS><CR> after last <CR>
- [x] For ASTM: convert each line to ASTM segment with segment number and checksum

### General
- [x] Update SPECIFICATION.md as features are designed, implemented, or changed
- [ ] Test all features and edge cases
- [ ] Document usage and protocols supported

---
_Last updated: February 27, 2026_
