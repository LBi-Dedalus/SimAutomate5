import { logError } from "./log.js";

const CONFIG_KEY = "simautomate:config";

window.addEventListener("DOMContentLoaded", init);

// Only fields explicitly marked with the `data-persist` attribute (and a name) are
// stored. Template editor, search and variable inputs are intentionally not marked.
const PERSISTED_FIELDS =
  "input[data-persist][name], select[data-persist][name], textarea[data-persist][name]";

function init() {
  hydrateConfig();
  setupConfigPersistence();
}

function persistedField(name) {
  for (const field of document.querySelectorAll(PERSISTED_FIELDS)) {
    if (field.name === name) return field;
  }
  return null;
}

function hydrateConfig() {
  try {
    const raw = localStorage.getItem(CONFIG_KEY);
    if (!raw) return;
    const data = JSON.parse(raw);

    for (const key in data) {
      if (key === "") continue;
      const field = persistedField(key);
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

function setupConfigPersistence() {
  for (const field of document.querySelectorAll(PERSISTED_FIELDS)) {
    field.addEventListener("change", () =>
      persistConfig(
        field.name,
        field.getAttribute("type") === "checkbox" ? field.checked : field.value,
      ),
    );
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
    logError(`Failed to persist config: ${String(err)}`, "config.js:persistConfig");
  }
}
