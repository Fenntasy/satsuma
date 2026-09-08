import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const FORWARDED_EVENTS = [
  "library://scan-progress",
  "library://scan-finished",
  "player://state",
];

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

export async function start(Elm, node) {
  // Register the event listeners before Elm can issue its first command,
  // otherwise a scan that finishes immediately is never reported.
  let app = null;
  const buffered = [];
  const deliver = (message) => {
    if (app) {
      app.ports.fromJs.send(message);
    } else {
      buffered.push(message);
    }
  };

  await Promise.all(
    FORWARDED_EVENTS.map((name) =>
      listen(name, (event) => deliver({ tag: "event", name, payload: event.payload })).catch(
        (error) => console.warn("[bridge] cannot listen to", name, error),
      ),
    ),
  );

  app = Elm.Main.init({
    node,
    flags: { theme: readTheme(), systemDark: darkQuery.matches },
  });
  for (const message of buffered) {
    app.ports.fromJs.send(message);
  }

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
    deliver({ tag: "systemTheme", dark: event.matches });
  });

  return app;
}
