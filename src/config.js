const CONFIG_KEY = "simautomate:config";

window.addEventListener("DOMContentLoaded", init);

function init() {
    hydrateConfig();
    setupConfigPersistence();
}

function hydrateConfig() {
    try {
        const raw = localStorage.getItem(CONFIG_KEY);
        if (!raw) return;
        const data = JSON.parse(raw);

        for (const key in data) {
            const field = document.querySelector(`[name="${key}"]`);
            if (!field) continue;
            if (field.type === "checkbox") {
                field.checked = data[key];
            } else {
                field.value = data[key];
            }
        }
    } catch (err) {
        console.error("Failed to hydrate config", err);
        void logError(`Failed to hydrate config: ${String(err)}`, "main.js:hydrateConfig");
    }
}

function setupConfigPersistence() {
    const fields = document.querySelectorAll("input, select, textarea");

    for (const field of fields) {
        field.addEventListener("change", ev => persistConfig(field.name, field.type === "checkbox" ? field.checked : field.value));
    }
}

function persistConfig(name, value) {
    try {
        const raw = localStorage.getItem(CONFIG_KEY);
        const data = raw ? JSON.parse(raw) : {};
        data[name] = value;
        localStorage.setItem(CONFIG_KEY, JSON.stringify(data));
    } catch (err) {
        console.error("Failed to persist config", err);
    }
}