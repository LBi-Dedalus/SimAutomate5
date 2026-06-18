const initialMessage =
  "\x02H|^~\\&|27_78_200911104853.hps|A5-AUT-0020|||ORM|||||P|H2.2|20200911104853";

Bun.listen({
  hostname: "127.0.0.1",
  port: 5000,
  socket: {
    open(socket) {
      console.log("client connected");
      socket.write(initialMessage);
    },
    data(socket, data) {
      const text = new TextDecoder().decode(data);
      console.log("received bytes=", data.length);
      let chars = [];
      for (let i = 0; i < text.length; i++) {
        const code = text.charCodeAt(i);
        if (code <= 0x1f) {
          chars.push("\\x" + code.toString(16));
        } else {
          chars.push(text.charAt(i));
        }
      }
      console.log(chars.join(""));
      socket.write("\x06");
    },
    close() {
      console.log("client disconnected");
    },
    error(socket, err) {
      console.error(err);
    },
  },
});
console.log("TCP server listening on 127.0.0.1:5000");
