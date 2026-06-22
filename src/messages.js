const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const MESSAGE_EVENT = "message://stream";

document.addEventListener("DOMContentLoaded", async () => {
  initMessageForm();
  await initChat();
  unlockMessageInputWhenConnected();
});

function initMessageForm() {
  const messageForm = document.getElementById("message-form");
  messageForm.addEventListener("submit", async (ev) => {
    ev.preventDefault();

    const message = messageForm.message.value;
    try {
      console.log("Sending message", message);
      await invoke("send_message", { payload: { message } });
    } catch (err) {
      console.error("Failed to send message", err);
    }
  });
}

async function initChat() {
  const clearButton = document.getElementById("clear-chat");
  clearButton.addEventListener("click", () => {
    const messagesEl = document.getElementById("messages");
    const messages = messagesEl.querySelectorAll(".message");
    messages.forEach((msgEl) => msgEl.remove());

    const noConnectionEl = document.getElementById("no-connection");
    if (noConnectionEl) {
      noConnectionEl.classList.remove("hidden");
    }
  });

  await listen(MESSAGE_EVENT, (event) => appendMessage(event.payload));
}

function unlockMessageInputWhenConnected() {
  window.connection_status.subscribe((status) => {
    const enable = ["connected"].includes(status);

    const messageForm = document.getElementById("message-form");
    const sendButton = messageForm.querySelector('button[type="submit"]');
    if (sendButton) {
      sendButton.disabled = !enable;
    }
  });
}

function appendMessage(payload) {
  const { msg_type, content, timestamp } = payload;

  const entry = document.createElement("div");
  entry.className = `message text-small px-4 py-1 ${msg_type}`;

  const type = document.createElement("div");
  type.className = "msg_type";
  type.textContent = formatType(msg_type);
  entry.appendChild(type);

  const time = document.createElement("span");
  time.className = "msg_time";
  time.textContent = formatTime(timestamp);
  entry.appendChild(time);

  const body = document.createElement("div");
  body.className = "message-body";
  {
    const contentLines = content.split("<CR>");
    for (const lineIdx in contentLines) {
      let line = contentLines[lineIdx];

      if (Number(lineIdx) + 1 !== contentLines.length) {
        line += "<CR>";
      } else if (line === "") {
        break;
      }

      const lineEl = document.createElement("p");
      lineEl.textContent = line;
      body.appendChild(lineEl);
    }
  }
  body.title = content;
  entry.appendChild(body);

  const messagesEl = document.getElementById("messages");
  const noConnectionEl = document.getElementById("no-connection");
  if (noConnectionEl) {
    noConnectionEl.classList.add("hidden");
  }

  messagesEl.appendChild(entry);
  messagesEl.scrollTop = messagesEl.scrollHeight;
  void logInfo(
    `Message appended (direction=${direction}, length=${content.length}, timestamp=${timestamp})`,
    "main.js:appendMessage",
  );
}

function formatTime(value) {
  try {
    return new Date(value).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      //   fractionalSecondDigits: 3
    });
  } catch (_) {
    return value;
  }
}

function formatType(msg_type) {
  switch (msg_type) {
    case "systeminfo":
      return "INF";
    case "systemwarn":
      return "WRN";
    case "systemerror":
      return "ERR";
    case "received":
      return "RCV";
    case "sent":
      return "SND";
    default:
      return "???";
  }
}
