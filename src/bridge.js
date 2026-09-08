import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const FORWARDED_EVENTS = ["library://scan-progress", "library://scan-finished"];

const THEME_KEY = "satsuma.theme";
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

function readTheme() {
  try {
    return localStorage.getItem(THEME_KEY);
  } catch {
    return null;
  }
}

function saveTheme(value) {
  try {
    localStorage.setItem(THEME_KEY, value);
  } catch {
    // Storage may be unavailable; the in-memory setting still applies.
  }
}

async function handleInvoke(app, { command, args }) {
  try {
    const payload = await invoke(command, args);
    app.ports.fromJs.send({ tag: "invokeResult", command, ok: true, payload });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    app.ports.fromJs.send({ tag: "invokeResult", command, ok: false, error: message });
  }
}

export function start(Elm, node) {
  const app = Elm.Main.init({
    node,
    flags: { theme: readTheme(), systemDark: darkQuery.matches },
  });

  app.ports.toJs.subscribe((message) => {
    switch (message.tag) {
      case "invoke":
        handleInvoke(app, message);
        break;
      case "saveTheme":
        saveTheme(message.value);
        break;
      default:
        console.warn("[bridge] unknown outgoing tag", message.tag);
    }
  });

  darkQuery.addEventListener("change", (event) => {
    app.ports.fromJs.send({ tag: "systemTheme", dark: event.matches });
  });

  for (const name of FORWARDED_EVENTS) {
    listen(name, (event) => {
      app.ports.fromJs.send({ tag: "event", name, payload: event.payload });
    }).catch((error) => console.warn("[bridge] cannot listen to", name, error));
  }

  return app;
}
