import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
  info: ErrorInfo | null;
}

/**
 * Catches render/lifecycle throws in the subtree so a single broken panel
 * surfaces a readable message instead of unmounting the whole React root (the
 * "black screen" failure mode — there is no other top-level boundary). The
 * stack is shown inline (this is a dev-facing desktop app) and logged to the
 * console for the WebView devtools / `tauri dev` capture.
 *
 * Not translated on purpose: i18n itself may be the thing that threw, and a
 * crash screen must never depend on the subsystem it might be reporting.
 */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null, info: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    this.setState({ info });
    // eslint-disable-next-line no-console
    console.error("[ErrorBoundary]", error, info.componentStack);
  }

  private reset = () => this.setState({ error: null, info: null });

  override render(): ReactNode {
    const { error, info } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="flex h-full w-full items-center justify-center overflow-auto bg-neutral-950 p-6">
        <div className="w-full max-w-2xl rounded-xl border border-red-900/60 bg-red-950/20 p-5 text-[12px] text-red-100">
          <h1 className="mb-2 text-sm font-semibold text-red-200">
            Something crashed
          </h1>
          <p className="mb-3 text-red-300/80">
            A panel hit an unrecoverable error. The rest of the app kept
            running — try again, and report this if it persists.
          </p>
          <pre className="mb-3 max-h-48 overflow-auto whitespace-pre-wrap rounded border border-red-900/50 bg-black/40 p-2 text-[11px] text-red-200">
            {error.message}
            {error.stack ? `\n\n${error.stack}` : ""}
            {info?.componentStack ? `\n\nComponent stack:${info.componentStack}` : ""}
          </pre>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={this.reset}
              className="rounded bg-red-900/50 px-3 py-1.5 font-medium text-red-100 ring-1 ring-inset ring-red-800/60 hover:bg-red-900/70"
            >
              Dismiss
            </button>
            <button
              type="button"
              onClick={() => window.location.reload()}
              className="rounded bg-neutral-800 px-3 py-1.5 font-medium text-neutral-200 hover:bg-neutral-700"
            >
              Reload app
            </button>
          </div>
        </div>
      </div>
    );
  }
}
