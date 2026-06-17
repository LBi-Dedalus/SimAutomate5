const { invoke } = window.__TAURI__.core;

window.addEventListener("DOMContentLoaded", init);

function init() {
  initClientConnect();
  initServerConnect();
}

function initClientConnect() {
  const form = document.querySelector("#client-options form");
  if (!form) return;

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();

    if (["connecting", "connected"].includes(window.connection_status.get())) {
      try {
        console.log(`Disconnect requested`);
        await invoke("disconnect_socket");
      } catch (err) {
        console.error("Failed to disconnect", err);
      }
      return;
    }

    const host = form.host.value;
    const port = Number(form.port.value);
    try {
      console.log(`Connect requested (host=${host}, port=${port})`);
      await invoke("connect_socket", {
        req: { type: "ClientConnectRequest", ip: host, port },
      });
    } catch (err) {
      console.error("Failed to connect", err);
    }
  });
}

function initServerConnect() {
  const form = document.querySelector("#server-options form");
  if (!form) return;

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();

    if (["connecting", "connected"].includes(window.connection_status.get())) {
      try {
        console.log(`Server stop requested`);
        await invoke("disconnect_socket");
      } catch (err) {
        console.error("Failed to stop server", err);
      }
      return;
    }

    const port = Number(form["server-port"].value);
    try {
      console.log(`Server start requested (port=${port})`);
      await invoke("connect_socket", {
        req: { type: "ServerStartRequest", port },
      });
    } catch (err) {
      console.error("Failed to start server", err);
    }
  });
}
