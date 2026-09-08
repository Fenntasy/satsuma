import { defineConfig } from "vite";
import elm from "vite-plugin-elm";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  // The Elm debugger opens a separate window, which the Tauri webview
  // refuses, and the exception takes the app down on startup.
  plugins: [elm({ debug: false })],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
