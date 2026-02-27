# Prompts

The goal of this document is to keep a trace of the prompts I tried

## Specifications

### Plan mode

> This is a Tauri application using Vanilla JS in the frontend. I want you to help me plan in order to successfully develop my application. Create a SPECIFICATION.md file in the root of the project where you will be able to create and update specifications for the project. This project is an application meant to send and receive messages with another application in the local network. There is no need for DNS resolution since we will be using IPs directly. The IP address and port of the application to join must be configurable. When both the IP and port are specified, a connect button should be enabled. When this button is clicked, a socket connection should be initiated using the provided IP and port. There should be an area for message display with received messages displayed in red and sent messages displayed in green. Each message should have a timestamp associated. There should be an input textarea where the user can enter text freely. There should be a "Clear" button next to both the message display and input areas. There should be a "Send" button next to the input area.

**Should actually have been in Agent mode I think !**

### Agent mode

> Use Tauri channels to communicate between the frontend and backend. Adapt the spec accordingly

> On click on the connect button, the application should open a TCP socket stream. It should try connecting with a timeout of 1 second. There should be an option to allow retries. If the option is activated, if the connection fails, we should retry up to 5 times with exponential backoff. Adapt the spec accordingly

> Exchanged messages will be mostly ASTM and HL7 messages but other formats could be supported. As such, messages have to be plain text. The app should be able to support special characters (for example <STX> and <EOT> for the ASTM protocol). It should automatically translate strings such as "<VT>", "<STX>" and others to the corresponding character. Adapt the spec accordingly

> There should be a "Message Builder" page. It should support ASTM and MLLP. There should be an input textarea and an output area with a "Build" button. On click on the build button, the input should be taken line by line and converted to an ASTM or MLLP message based on the selected output protocol. If MLLP is selected a "<VT>" string is added at the beginning, "<CR>" is added at the end of each line and "<FS><CR>" is added after the last "<CR>". If ASTM is selected, each line is converted to an ASTM segment with the correct segment number and checksum. Adapt the spec accordingly

> Remove the tasks checklist and create a TODO.md file containing a detailed todo list for this application.

> I want the application to automatically respond to incoming messages. The supported protocols for automatic responses are ASTM and HL7. For each option, we should be able to configure the response. For ASTM, the response message would be given by the user. For HL7, the user would give a message type and a response code and the program would generate a response message from this configuration and the received message. There should be a toggle to enable or disable automatic responses. Adapt the spec and the todo list accordingly

## Wireframe

### Agent mode

> Start designing a UI of the application. I will give you feedback on the resulting design. You can use the Oat (https://oat.ink) component library but no other component / CSS library. If a JS library is required, ask before doing anything.

> I like having the configuration located at the top of the window, the output in the center and the input at the bottom, similar to a chat app. However, the input should be larger.


> Do not use web-components, but rather semantic HTML to build the interface. This is how the Oat library works. Update the specifications and instructions accordingly

**Switch to Claude Sonnet 4.6 from GPT 4.1**

> Dark theme is broken. I think you didn't use the correct CSS variables. Here are the variables used by Oat : 

```css
/* Page background */
--background: rgb(255 255 255);

/* Primary text color */
--foreground: rgb(9 9 11);

/* Card background */
--card: rgb(255 255 255);

/* Card text color */
--card-foreground: rgb(9 9 11);

/* Primary buttons and links */
--primary: rgb(24 24 27);

/* Text color on primary buttons */
--primary-foreground: rgb(250 250 250);

/* Secondary button background */
--secondary: rgb(244 244 245);

/* Text colour on secondary buttons */
--secondary-foreground: rgb(24 24 27);

/* Muted (lighter) background */
--muted: rgb(244 244 245);

/* Muted (lighter) text colour */
--muted-foreground: rgb(113 113 122);

/* Subtler than muted background */
--faint: rgb(250 250 250);

/* Subtler than muted text color */
--faint-foreground: rgb(161 161 170);

/* Accent background */
--accent: rgb(244 244 245);

/* Accent text color */
--accent-foreground: rgb(24 24 27);

/* Error/danger color */
--danger: rgb(223 81 76);

/* Text color on danger background */
--danger-foreground: rgb(250 250 250);

/* Success color */
--success: rgb(76 175 80);

/* Text colour on success background */
--success-foreground: rgb(250 250 250);

/* Warning color */
--warning: rgb(255 140 0);

/* Text colour on warning background */
--warning-foreground: rgb(9 9 11);

/* Border color (boxes) */
--border: rgb(212 212 216);

/* Input borders */
--input: rgb(212 212 216);

/* Focus ring color */
--ring: rgb(24 24 27);
```

> This is very good. Can you move the auto-response configuration part to the right of the connect button and add a way to easily see if the connection has been established or not ? Update specs and todos

> Update the wireguard to move the protocol select to the right too. The configuration for auto response should also be on the right and depend on the selected protocol. MLLP should not be an option in this select. Add a toggle which should be on by default to enable or disable connection retries. Can you also make the inputs more compact ?

> Great ! Can you display the config-left and config-right panel with 2 lines each ? Also transform the astm response input from a textarea to an input

## Backend

### Agent mode

> I want you to build the backend for this application. Read the specifications and create the Rust code for it. Limit yourself to the backend / Rust code. Make sure to update the TODO file

> I want you to build the backend for this application. Read the specifications and create the Rust code for it. Limit yourself to the backend / Rust code. Make sure to update the TODO file

## Linking both 

### Agent mode

> You have written the backend code. I have updated it a bit, but haven't changed the stucture of the code or any public function signature. Can you now link the frontend and the backend of this Tauri app ? Make sure to update the specs if needed and the todo accordingly

## Bugfixing

### Agent mode

> This is looking good. I now want you to fix some stuff. First, the build output isn't editable, which means I can't send my own custom mesages. Second, I want special characters to be displayed there as human readable strings, similar to what is done in the message display section. Third, I want the select for the auto response protocol to be larger. Make sure to update the spec and todo accordingly

> The disconnect behaviour isn't what is expected. On disconnect, the socket should close cleanly. It is not necessary to close the connection cleanly on application close. Update the spec and todo accordingly

## Extra features

### Agent mode

> I want you to add extra features. The first one is persistent configuration. The configuration should be saved (whether in an JSON file or in another easily accessible place like localStorage) whenever a config change is made and retrieved whenever the app launches. The second extra feature I want is autobuild. Instead of having 2 textareas, one for input and one for output, a protocol select and a Build button, I want a button called "Autobuild". This button would send the inputted message to the backend which would detect if the message is an ASTM message (starting with "H|") or an HL7 message (starting with "MSH|") and call the corresponding BuildMessage function (ASTM for ASTM, MLLP for HL7). The generated output would replace the user input. If the message isn't ASTM or HL7, leave the user input intact. Make sure to update the specs and todo 