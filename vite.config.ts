import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// Caduceus ships three separate webviews (staff / command center / settings), so Vite
// is configured as a multi-page app. Each HTML file at the repo root is an entry
// point that Tauri loads by path (see src-tauri/tauri.conf.json + window/mod.rs).
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
        settings: resolve(__dirname, "settings.html"),
        chat: resolve(__dirname, "chat.html"),
        manage: resolve(__dirname, "manage.html"),
      },
    },
  },

  resolve: {
    alias: {
      "@": resolve(__dirname, "src"),
    },
  },
});
