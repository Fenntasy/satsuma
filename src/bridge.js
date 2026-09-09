import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const FORWARDED_EVENTS = [
  "library://scan-progress",
  "library://scan-finished",
  "player://state",
];

const THEME_KEY = "satsuma.theme";
const WIDTHS_KEY = "satsuma.columnWidths";
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

function readWidths() {
  try {
    const stored = localStorage.getItem(WIDTHS_KEY);
    return stored === null ? {} : JSON.parse(stored);
  } catch {
    return {};
  }
}

function saveWidths(widths) {
  try {
    localStorage.setItem(WIDTHS_KEY, JSON.stringify(widths));
  } catch {
    // Widths are a convenience; failing to keep them is not worth a fuss.
  }
}

/**
 * Turns a drag of a column grip into a `resized` event carrying how far it
 * moved, which is all Elm needs to work out the new width.
 */
function installColumnResizing(root) {
  let dragging = null;

  root.addEventListener("mousedown", (event) => {
    const grip = event.target.closest(".column-grip");
    if (grip === null) {
      return;
    }
    dragging = { grip, from: event.clientX };
    event.preventDefault();
  });

  window.addEventListener("mousemove", (event) => {
    if (dragging === null) {
      return;
    }
    const delta = Math.round(event.clientX - dragging.from);
    if (delta === 0) {
      return;
    }
    dragging.from = event.clientX;
    dragging.grip.dispatchEvent(
      new CustomEvent("resized", { detail: { delta }, bubbles: false }),
    );
  });

  window.addEventListener("mouseup", () => {
    dragging = null;
  });
}

/**
 * A command rejects with whatever the backend returned, which is a string
 * today but need not be: anything else is shown as JSON rather than as
 * "[object Object]".
 */
function describeError(error) {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  try {
    // `JSON.stringify` answers `undefined` rather than throwing for some
    // values, and the port needs a string.
    return JSON.stringify(error) ?? String(error);
  } catch {
    return String(error);
  }
}

async function handleInvoke(app, { command, args }) {
  try {
    const payload = await invoke(command, args);
    app.ports.fromJs.send({ tag: "invokeResult", command, ok: true, payload });
  } catch (error) {
    app.ports.fromJs.send({
      tag: "invokeResult",
      command,
      ok: false,
      error: describeError(error),
    });
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
    flags: {
      theme: readTheme(),
      systemDark: darkQuery.matches,
      widths: readWidths(),
    },
  });
  installColumnResizing(node);
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
      case "saveWidths":
        saveWidths(message.value);
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
