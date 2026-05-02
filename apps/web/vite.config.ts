import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// Tauri expects a fixed dev server port (1420 by default) and watches the
// frontend dist for production builds. The dev URL is referenced from
// `apps/desktop/tauri.conf.json` (`build.devUrl`).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "~": path.resolve(__dirname, "./src") },
  },
  // Tauri uses Chromium on Windows (WebView2) — no need to support older targets.
  build: {
    target: "esnext",
    sourcemap: true,
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      ignored: ["**/target/**", "**/apps/desktop/**"],
    },
  },
});
