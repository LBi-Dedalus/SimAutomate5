
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
        if (window.isConnected.get()) return;
        clientModeBtn.classList.remove("ghost");
        serverModeBtn.classList.add("ghost");
        clientOptions.classList.remove("hidden");
        serverOptions.classList.add("hidden");
    });

    serverModeBtn.addEventListener("click", () => {
        if (window.isConnected.get()) return;
        serverModeBtn.classList.remove("ghost");
        clientModeBtn.classList.add("ghost");
        serverOptions.classList.remove("hidden");
        clientOptions.classList.add("hidden");
    });
}

function lockFormsWhenConnected() {
    window.isConnected.subscribe(connected => {
        const clientModeBtn = document.getElementById("client-mode");
        const serverModeBtn = document.getElementById("server-mode");

        const clientFormFields = clientModeBtn.querySelectorAll("input, select, textarea, button");
        for (const field of clientFormFields) {
            field.disabled = connected;
        }

        const serverFormFields = serverModeBtn.querySelectorAll("input, select, textarea, button");
        for (const field of serverFormFields) {
            field.disabled = connected;
        }
    });
}
