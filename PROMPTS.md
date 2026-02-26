# Prompts

The goal of this document is to keep a trace of the prompts I tried

## Specifications

### Plan mode

    This is a Tauri application using Vanilla JS in the frontend. I want you to help me plan in order to successfully develop my application. Create a SPECIFICATION.md file in the root of the project where you will be able to create and update specifications for the project. This project is an application meant to send and receive messages with another application in the local network. There is no need for DNS resolution since we will be using IPs directly. The IP address and port of the application to join must be configurable. When both the IP and port are specified, a connect button should be enabled. When this button is clicked, a socket connection should be initiated using the provided IP and port. There should be an area for message display with received messages displayed in red and sent messages displayed in green. Each message should have a timestamp associated. There should be an input textarea where the user can enter text freely. There should be a "Clear" button next to both the message display and input areas. There should be a "Send" button next to the input area.

> Should actually have been in Agent mode I think !

### Agent mode

    Use Tauri channels to communicate between the frontend and backend. Adapt the spec accordingly

    On click on the connect button, the application should open a TCP socket stream. It should try connecting with a timeout of 1 second. There should be an option to allow retries. If the option is activated, if the connection fails, we should retry up to 5 times with exponential backoff. Adapt the spec accordingly

    Exchanged messages will be mostly ASTM and HL7 messages but other formats could be supported. As such, messages have to be plain text. The app should be able to support special characters (for example <STX> and <EOT> for the ASTM protocol). It should automatically translate strings such as "<VT>", "<STX>" and others to the corresponding character. Adapt the spec accordingly

    There should be a "Message Builder" page. It should support ASTM and MLLP. There should be an input textarea and an output area with a "Build" button. On click on the build button, the input should be taken line by line and converted to an ASTM or MLLP message based on the selected output protocol. If MLLP is selected a "<VT>" string is added at the beginning, "<CR>" is added at the end of each line and "<FS><CR>" is added after the last "<CR>". If ASTM is selected, each line is converted to an ASTM segment with the correct segment number and checksum. Adapt the spec accordingly

    Remove the tasks checklist and create a TODO.md file containing a detailed todo list for this application.

    I want the application to automatically respond to incoming messages. The supported protocols for automatic responses are ASTM and HL7. For each option, we should be able to configure the response. For ASTM, the response message would be given by the user. For HL7, the user would give a message type and a response code and the program would generate a response message from this configuration and the received message. There should be a toggle to enable or disable automatic responses. Adapt the spec and the todo list accordingly