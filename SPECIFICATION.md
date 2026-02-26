# SPECIFICATION.md

## Project Overview
This application is a Tauri-based desktop app with a Vanilla JS frontend. Its purpose is to send and receive messages with another application on the local network. Communication is direct via IP address and port; DNS resolution is not required.

## Functional Requirements

### Configuration
- User can enter the IP address and port of the remote application.
- Both fields must be filled for the Connect button to be enabled.
- The IP and port are configurable at any time.


### Connection
- When Connect is clicked, the application attempts to open a TCP socket stream to the provided IP and port.
- The connection attempt uses a timeout of 1 second.
- There is an option to allow connection retries (toggle in the UI).
- If retries are enabled and the connection fails, the application retries up to 5 times with exponential backoff (e.g., 1s, 2s, 4s, 8s, 16s).

### Messaging UI
- Message display area:
  - Received messages are shown in red.
  - Sent messages are shown in green.
  - Each message includes a timestamp.
  - A Clear button allows clearing the message display area.
- Input area:
  - Textarea for entering messages.
  - Send button to send the message.
  - Clear button to clear the input area.

### Automatic Responses
- The application can automatically respond to incoming messages for ASTM and HL7 protocols.
- There is a toggle to enable or disable automatic responses.
- For ASTM:
  - The user configures the response message to send automatically.
- For HL7:
  - The user configures the message type and response code.
  - The program generates a response message based on this configuration and the received message.


## Message Builder

### Overview
The application will include a "Message Builder" page to construct messages for ASTM and MLLP protocols.

### Features
- Input textarea for entering message lines.
- Output area to display the built message.
- Protocol selection (ASTM or MLLP).
- "Build" button to generate the message.

### Build Logic
- On clicking the Build button:
  - The input is processed line by line.
  - If MLLP is selected:
    - "<VT>" is added at the beginning.
    - "<CR>" is added at the end of each line.
    - "<FS><CR>" is added after the last "<CR>".
  - If ASTM is selected:
    - Each line is converted to an ASTM segment with the correct segment number and checksum.
    - The output displays the constructed ASTM message.

## Technical Requirements
- Tauri backend will handle TCP socket communication, including connection timeout and retry logic with exponential backoff.
- Frontend will use Vanilla JS for UI logic, including the retry option toggle.
- Communication between frontend and backend will use Tauri channels (event/message system) for sending and receiving messages, connection status, and errors.
- UI must provide configuration for automatic responses (toggle, ASTM response message, HL7 message type and response code).
- Backend must handle automatic response logic for ASTM and HL7, generating and sending responses as configured.
- All features must be implemented from scratch.


## Message Format and Protocol Support
- Messages are plain text to support ASTM and HL7 protocols, as well as other formats.
- The app must support special characters (e.g., <STX>, <EOT>, <VT>, etc.) required by ASTM and HL7.
- The app will automatically translate protocol strings such as "<VT>", "<STX>", and others to their corresponding character values when sending and displaying messages.

## Open Questions


