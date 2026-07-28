import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// Caduceus ships several webviews: the always-on staff, the recording HUD
// that exists only while a microphone is live, the Command Center that holds
// everything else as tabs, and any number of floating widgets (all sharing
// this one `widget.html` entry — see src-tauri/src/widgets.rs for how each
// instance is told apart at runtime). Each HTML file at the repo root is an
// entry point Tauri loads by path (see src-tauri/tauri.conf.json + window/mod.rs).
export default defineConfig({
  plugins: [react()],

  // Tauri expects a fixed port and fails the build if it is not available.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      // Rust sources are rebuilt by cargo, not Vite.
      ignored: ["**/src-tauri/**", "**/website/**"],
    },
  },

  // `TAURI_ENV_*` vars are injected by the Tauri CLI during `tauri dev/build`.
  envPrefix: ["VITE_", "TAURI_ENV_"],

  build: {
    // Safari 13+/Chromium 89+ covers every webview Tauri v2 targets.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    rollupOptions: {
      input: {
        staff: resolve(__dirname, "index.html"),
        commandCenter: resolve(__dirname, "command-center.html"),
        recorder: resolve(__dirname, "recorder.html"),
        widget: resolve(__dirname, "widget.html"),
        // The Highlight & Act PopBar — see src-tauri/src/popbar.rs. Its own
        // dynamically created window, the same reason `widget` is here
        // rather than in tauri.conf.json's static `windows` array.
        popbar: resolve(__dirname, "popbar.html"),
        // The meeting notes pop-out — see src-tauri/src/meeting.rs. Same
        // dynamically-created-window story as `widget` and `popbar`: it has
        // no entry in tauri.conf.json's static `windows` array either.
        meeting: resolve(__dirname, "meeting.html"),
      },
    },
  },

  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },
});
