const { listen } = window.__TAURI__.event;

const MESSAGE_EVENT = "message://stream";

document.addEventListener("DOMContentLoaded", async () => {
  await listen(MESSAGE_EVENT, (event) => appendMessage(event.payload));
});

function appendMessage(payload) {
  const { direction, content, timestamp } = payload;
  const entry = document.createElement("div");
  entry.className = `message ${direction === "sent" ? "sent" : "received"}`;

  const time = document.createElement("span");
  time.className = "text-muted";
  time.textContent = formatTime(timestamp);
  entry.appendChild(time);

  const body = document.createElement("div");
  body.className = "message-body";
  body.textContent = content.replaceAll("<CR>", "<CR>\n");
  entry.appendChild(body);

  const messagesEl = document.getElementById("message");
  const noConnectionEl = document.getElementById("no-connection");
  if (noConnectionEl) {
    noConnectionEl.remove();
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
