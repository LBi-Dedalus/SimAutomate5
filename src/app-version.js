// Fills the sidebar footer with the running app's version, read at startup (never copied into the page).
import { logError } from "./log.js";

export const VERSION_UNAVAILABLE = "Version unavailable";

export async function showAppVersion({ getVersion, element, reportError = logError }) {
  try {
    element.textContent = `SimAutomate v${await getVersion()}`;
  } catch (error) {
    element.textContent = VERSION_UNAVAILABLE;
    reportError(`Reading the app version failed: ${error?.message ?? error}`, "app-version");
  }
}

const getVersion = globalThis.window?.__TAURI__?.app?.getVersion;
const element = globalThis.document?.getElementById?.("app-version");
if (element) {
  showAppVersion({
    getVersion: getVersion ?? (() => Promise.reject(new Error("Tauri app API is unavailable"))),
    element,
  });
}
