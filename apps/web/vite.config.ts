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
    // Manual chunk splits keep the initial JS small. Monaco is already lazy-
    // loaded via dynamic import; these splits move the rest of the heavy
    // libs into their own files so first paint isn't blocked on them.
    rollupOptions: {
      output: {
        // Rolldown-flavored Vite expects a function here. Map id → chunk
        // name; return undefined to let the bundler decide.
        manualChunks(id: string): string | undefined {
          if (!id.includes("node_modules")) return undefined;
          if (id.includes("monaco-editor")) return "vendor-monaco";
          if (
            id.includes("@codemirror") ||
            id.includes("codemirror") ||
            id.includes("@lezer")
          ) {
            return "vendor-codemirror";
          }
          if (id.includes("react-markdown") || id.includes("remark-")) {
            return "vendor-markdown";
          }
          if (id.includes("shiki")) return "vendor-shiki";
          if (id.includes("@xterm")) return "vendor-xterm";
          if (
            /[\\/]node_modules[\\/](react|react-dom|react-i18next|i18next|i18next-browser-languagedetector|scheduler)[\\/]/
              .test(id)
          ) {
            return "vendor-react";
          }
          if (id.includes("@tauri-apps")) return "vendor-tauri";
          return undefined;
        },
      },
    },
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
