const { listen } = window.__TAURI__.event;

const MESSAGE_EVENT = "message://stream";

document.addEventListener("DOMContentLoaded", async () => {
  await listen(MESSAGE_EVENT, (event) => appendMessage(event.payload));
});

function appendMessage(payload) {
  const { msg_type, content, timestamp } = payload;
  const entry = document.createElement("div");
  entry.className = `message text-small px-4 py-2`;

  const time = document.createElement("span");
  time.className = "text-muted";
  time.textContent = formatTime(timestamp);
  entry.appendChild(time);

  const type = document.createElement("span");
  type.className = msg_type;
  type.textContent = formatType(msg_type);
  entry.appendChild(type);

  const body = document.createElement("span");
  body.className = "message-body";
  body.textContent = content.replaceAll("<CR>", "<CR>\n");
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
      return "REC";
    case "sent":
      return "SND";
    default:
      return "???";
  }
}
