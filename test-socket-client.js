const host = "localhost";
const port = 8888;
const initialMessage = "\x02H|\\&^|...";

const decoder = new TextDecoder();

function formatBytes(data) {
  const text = decoder.decode(data);
  const parts = [];

  for (let index = 0; index < text.length; index++) {
    const code = text.charCodeAt(index);
    parts.push(
      code <= 0x1f ? `\\x${code.toString(16).padStart(2, "0")}` : text[index],
    );
  }

  return parts.join("");
}

await Bun.connect({
  hostname: host,
  port,
  socket: {
    open(socket) {
      console.log(`connected to ${host}:${port}`);
      socket.write(initialMessage);
    },
    data(_socket, data) {
      console.log(`received bytes=${data.length}`);
      console.log(formatBytes(data));
    },
    close() {
      console.log("client disconnected");
    },
    error(_socket, error) {
      console.error("socket error", error);
    },
  },
});
