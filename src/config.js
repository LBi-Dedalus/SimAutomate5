import { logError } from "./log.js";

const CONFIG_KEY = "simautomate:config";

// A field is persisted when its markup declares data-action="change:config.persist" and a name:
// that attribute stores it when edited, and the stored value is restored into the field with the
// same name at startup. Names are unique across the app.

function init() {
  hydrateConfig();
}

function hydrateConfig() {
  try {
    const raw = localStorage.getItem(CONFIG_KEY);
    if (!raw) return;
    const data = JSON.parse(raw);

    for (const key in data) {
      const field = document.querySelector(`[name="${key}"]`);
      if (!field) continue;
      if (field.getAttribute("type") === "checkbox") {
        field.checked = data[key];
      } else {
        field.value = data[key];
      }
    }
  } catch (err) {
    console.error("Failed to hydrate config", err);
    logError(`Failed to hydrate config: ${String(err)}`, "config.js:hydrateConfig");
  }
}

window.actions ??= {};
window.actions.config = {
  persist: (ev, field) =>
    persistConfig(field.name, field.getAttribute("type") === "checkbox" ? field.checked : field.value),
};

function persistConfig(name, value) {
  try {
    const raw = localStorage.getItem(CONFIG_KEY);
    const data = raw ? JSON.parse(raw) : {};
    data[name] = value;
    localStorage.setItem(CONFIG_KEY, JSON.stringify(data));
  } catch (err) {
    console.error("Failed to persist config", err);
    logError(`Failed to persist config: ${String(err)}`, "config.js:persistConfig");
  }
}

init();
