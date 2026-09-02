import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "~/App.tsx";
import { ErrorBoundary } from "~/components/ErrorBoundary.tsx";
import "~/i18n.ts";
import "~/index.css";

// The window runs with `dragDropEnabled: false` so the webview gets HTML5 drop
// events (composer attachments, sidebar reorder). The flip side: a file dropped
// anywhere *without* a handler makes Chromium navigate to its `file://` URL,
// which blows the SPA away. Swallow those at the document level; the composers'
// own handlers call `stopPropagation` before this ever sees them.
const swallowFileDrag = (e: DragEvent) => {
  if (Array.from(e.dataTransfer?.types ?? []).includes("Files")) {
    e.preventDefault();
  }
};
window.addEventListener("dragover", swallowFileDrag);
window.addEventListener("drop", swallowFileDrag);

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("#root not found");

ReactDOM.createRoot(rootEl).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
