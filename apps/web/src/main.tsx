import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "~/App.tsx";
import { ErrorBoundary } from "~/components/ErrorBoundary.tsx";
import "~/i18n.ts";
import "~/index.css";

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("#root not found");

ReactDOM.createRoot(rootEl).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
