/**
 * A stand-in for the Tauri host, injected before the app loads so the page
 * runs exactly as it does in the desktop app.
 *
 * `@tauri-apps/api` talks to the host through `window.__TAURI_INTERNALS__`:
 * `invoke` carries commands, and `listen` is itself a command that hands
 * over a callback id. Implementing those three functions is enough to drive
 * the real frontend without a Rust backend.
 */
export function installTauriStub(seed = {}) {
  const calls = [];
  const callbacks = new Map();
  const listeners = new Map();
  let nextCallbackId = 1;

  /** Replies for commands, seeded before the page loads. */
  const replies = new Map(Object.entries(seed));

  window.__TAURI_INTERNALS__ = {
    transformCallback(callback, once = false) {
      const id = nextCallbackId++;
      callbacks.set(id, { callback, once });
      return id;
    },

    unregisterCallback(id) {
      callbacks.delete(id);
    },

    convertFileSrc(path) {
      return path;
    },

    invoke(command, args) {
      // `listen` is a command like any other, and carries the callback id
      // the event should be delivered to.
      if (command === "plugin:event|listen") {
        const { event, handler } = args;
        listeners.set(event, handler);
        return Promise.resolve(nextCallbackId++);
      }
      if (command === "plugin:event|unlisten") {
        return Promise.resolve();
      }

      calls.push({ command, args });
      const reply = replies.get(command);
      if (reply === undefined) {
        return Promise.resolve(null);
      }
      if (reply instanceof Error) {
        return Promise.reject(reply);
      }
      return Promise.resolve(reply);
    },
  };

  window.__SATSUMA_TEST__ = {
    /** Every command the app has sent, in order. Reading does not clear
     * them: a poll that consumed a partial batch could never succeed. */
    calls() {
      return calls.slice();
    },

    /** Forgets the commands sent so far. */
    clearCalls() {
      calls.length = 0;
    },

    /** Makes `command` answer with `value`, or fail when given an Error. */
    reply(command, value) {
      replies.set(command, value);
    },

    /** Makes `command` fail with `message`, as a Tauri command error does. */
    failWith(command, message) {
      replies.set(command, new Error(message));
    },

    /** Delivers a backend event to the app. */
    emit(event, payload) {
      const handler = listeners.get(event);
      if (handler === undefined) {
        throw new Error(`nothing is listening to ${event}`);
      }
      const entry = callbacks.get(handler);
      if (entry === undefined) {
        throw new Error(`no callback registered for ${event}`);
      }
      entry.callback({ event, id: handler, payload });
    },

    /** Whether the app subscribed to an event. */
    listensTo(event) {
      return listeners.has(event);
    },
  };
}
