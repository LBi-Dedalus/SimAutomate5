
window.addEventListener("DOMContentLoaded", init);

function init() {
    initModeButtons();
    lockFormsWhenConnected();
}

function initModeButtons() {
    const clientModeBtn = document.getElementById("client-mode");
    const serverModeBtn = document.getElementById("server-mode");
    const clientOptions = document.getElementById("client-options");
    const serverOptions = document.getElementById("server-options");

    clientModeBtn.addEventListener("click", () => {
        const disable = ["connecting", "connected"].includes(window.status.get());
        if (disable) return;

        clientModeBtn.classList.remove("ghost");
        serverModeBtn.classList.add("ghost");
        clientOptions.classList.remove("hidden");
        serverOptions.classList.add("hidden");
    });

    serverModeBtn.addEventListener("click", () => {
        const disable = ["connecting", "connected"].includes(window.status.get());
        if (disable) return;

        serverModeBtn.classList.remove("ghost");
        clientModeBtn.classList.add("ghost");
        serverOptions.classList.remove("hidden");
        clientOptions.classList.add("hidden");
    });
}

function lockFormsWhenConnected() {
    window.status.subscribe(status => {
        const disable = ["connecting", "connected"].includes(status);
        const clientModeBtn = document.getElementById("client-mode");
        const serverModeBtn = document.getElementById("server-mode");

        const clientFormFields = clientModeBtn.querySelectorAll("input, select, textarea, button");
        for (const field of clientFormFields) {
            field.disabled = disable;
        }

        const serverFormFields = serverModeBtn.querySelectorAll("input, select, textarea, button");
        for (const field of serverFormFields) {
            field.disabled = disable;
        }
    });
}
