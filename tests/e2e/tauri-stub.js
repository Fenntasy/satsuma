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

  /** Commands that fail, and the value they fail with. */
  const rejections = new Map();

  window.__TAURI_INTERNALS__ = {
    transformCallback(callback, once = false) {
      const id = nextCallbackId++;
      callbacks.set(id, { callback, once });
      return id;
    },

    unregisterCallback(id) {
      callbacks.delete(id);
    },

    /**
     * The host turns a path into a URL on the given custom protocol. This
     * is the spelling macOS and Linux use; Windows uses
     * `http://<protocol>.localhost/<path>` instead, which is the whole
     * reason the frontend asks rather than building the URL itself.
     */
    convertFileSrc(path, protocol = "asset") {
      return `${protocol}://localhost/${encodeURIComponent(path)}`;
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
        listeners.delete(args?.event);
        return Promise.resolve();
      }

      calls.push({ command, args });
      if (rejections.has(command)) {
        // Tauri rejects with the value the command returned, not with an
        // Error, and a `Result<_, String>` therefore rejects with a string.
        return Promise.reject(rejections.get(command));
      }
      const reply = replies.get(command);
      return Promise.resolve(reply === undefined ? null : reply);
    },
  };

  // `@tauri-apps/api` calls this before it invokes `unlisten`.
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(event) {
      listeners.delete(event);
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

    /** Makes `command` answer with `value`. */
    reply(command, value) {
      replies.set(command, value);
    },

    /** Makes `command` fail with `value`, the way the host does. */
    failWith(command, value) {
      rejections.set(command, value);
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
