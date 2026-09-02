import type { Extension } from "@codemirror/state";
import { EditorView, hoverTooltip } from "@codemirror/view";
import { type Diagnostic, lintGutter, setDiagnostics } from "@codemirror/lint";
import {
  isUnsupported,
  lspDefinition,
  lspDiagnostics,
  lspFormat,
  lspHover,
  type LspLocation,
} from "~/ipc/lsp.ts";

/**
 * Language-server features for the file editor: diagnostics, hover,
 * go-to-definition and format.
 *
 * The servers are the ones `LspManager` already runs per worktree for the MCP
 * tools — nothing extra is spawned. Every call carries the live buffer, so
 * results track unsaved edits (and WSL projects work, where the desktop side
 * cannot read the distro's paths on its own).
 *
 * Failures are deliberately quiet. A missing server, an unsupported file type
 * or a server that died mid-session must never break typing, so each entry
 * point swallows its error and the feature simply produces nothing.
 */

export type LspContext = {
  projectId: string;
  worktreeId: string;
  relPath: string;
  /** Open a definition target that lives inside the worktree. */
  openLocation: (loc: LspLocation) => void;
};

/** LSP `{line, character}` → CodeMirror offset, clamped to the document.
 *  Diagnostics can lag the buffer by a debounce and point past its end. */
export function lspPosToOffset(
  doc: { lines: number; line: (n: number) => { from: number; to: number } },
  line: number,
  character: number,
): number {
  const lineNo = Math.min(Math.max(line + 1, 1), doc.lines);
  const l = doc.line(lineNo);
  return Math.min(l.from + character, l.to);
}

/** CodeMirror offset → LSP `{line, character}` (both 0-based on the wire). */
export function offsetToLspPos(
  doc: { lineAt: (pos: number) => { number: number; from: number } },
  offset: number,
): { line: number; character: number } {
  const l = doc.lineAt(offset);
  return { line: l.number - 1, character: offset - l.from };
}

const SEVERITY: Record<string, Diagnostic["severity"]> = {
  error: "error",
  warning: "warning",
  info: "info",
  hint: "hint",
};

/**
 * Pull the server's diagnostics for the current buffer and paint them.
 *
 * `publishDiagnostics` is a push notification, so the backend can only hand
 * back the last pass the server completed — a call right after an edit returns
 * the state before it. The caller re-runs on a debounce, which converges.
 */
export async function refreshLspDiagnostics(
  view: EditorView,
  ctx: LspContext,
): Promise<void> {
  const text = view.state.doc.toString();
  let diags;
  try {
    diags = await lspDiagnostics({ ...ctx, text });
  } catch (e) {
    // Unsupported file → no server covers it, nothing to show. Any other
    // failure (server down, crashed) also just leaves the editor unmarked.
    if (!isUnsupported(e)) console.debug("lsp diagnostics failed", e);
    return;
  }
  const doc = view.state.doc;
  const mapped: Diagnostic[] = diags.map((d) => {
    const from = lspPosToOffset(doc, d.start_line, d.start_character);
    const to = lspPosToOffset(doc, d.end_line, d.end_character);
    return {
      from,
      // A zero-width diagnostic renders as nothing; widen it by one so the
      // marker is visible (servers do emit empty ranges at EOF).
      to: to > from ? to : Math.min(from + 1, doc.length),
      severity: SEVERITY[d.severity] ?? "error",
      message: d.source ? `${d.source}: ${d.message}` : d.message,
    };
  });
  view.dispatch(setDiagnostics(view.state, mapped));
}

/**
 * Format the document through the server and apply the edits.
 *
 * Edits are resolved against the text that was submitted, so they are applied
 * as one transaction of pre-edit offsets — CodeMirror handles the shifting.
 * Returns false when nothing was applied (no server, no edits).
 */
export async function runLspFormat(
  view: EditorView,
  ctx: LspContext,
): Promise<boolean> {
  const text = view.state.doc.toString();
  let edits;
  try {
    edits = await lspFormat({
      ...ctx,
      text,
      tabSize: view.state.tabSize,
      insertSpaces: true,
    });
  } catch (e) {
    if (!isUnsupported(e)) console.debug("lsp format failed", e);
    return false;
  }
  if (edits.length === 0) return false;
  // The document may have changed while the request was in flight; applying
  // stale offsets would scramble it.
  if (view.state.doc.toString() !== text) return false;
  const doc = view.state.doc;
  view.dispatch({
    changes: edits.map((e) => ({
      from: lspPosToOffset(doc, e.start_line, e.start_character),
      to: lspPosToOffset(doc, e.end_line, e.end_character),
      insert: e.new_text,
    })),
  });
  return true;
}

/**
 * Static extensions: the lint gutter, hover tooltips and Ctrl/Cmd+click to
 * jump to a definition. Diagnostics themselves are pushed by
 * {@link refreshLspDiagnostics} rather than pulled by a `linter()` source, so
 * the editor controls when a request goes out.
 */
export function lspExtensions(ctx: LspContext): Extension[] {
  return [
    lintGutter(),
    hoverTooltip(async (view, pos) => {
      const { line, character } = offsetToLspPos(view.state.doc, pos);
      let text: string | null;
      try {
        text = await lspHoverSafe(view, ctx, line, character);
      } catch {
        return null;
      }
      if (!text) return null;
      return {
        pos,
        above: true,
        create: () => {
          const dom = document.createElement("div");
          dom.className = "cm-lsp-hover";
          // Servers answer in Markdown; rendering it raw in a <pre> keeps the
          // code blocks readable without pulling in a Markdown renderer.
          const pre = document.createElement("pre");
          pre.textContent = text;
          dom.appendChild(pre);
          return { dom };
        },
      };
    }),
    EditorView.domEventHandlers({
      mousedown: (event, view) => {
        if (!event.ctrlKey && !event.metaKey) return false;
        const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
        if (pos == null) return false;
        const { line, character } = offsetToLspPos(view.state.doc, pos);
        event.preventDefault();
        void jumpToDefinition(view, ctx, line, character);
        return true;
      },
    }),
    EditorView.theme({
      ".cm-lsp-hover": {
        maxWidth: "min(560px, 60vw)",
        maxHeight: "320px",
        overflow: "auto",
        padding: "6px 8px",
        background: "#141414",
        border: "1px solid #333",
        borderRadius: "4px",
      },
      ".cm-lsp-hover pre": {
        margin: 0,
        whiteSpace: "pre-wrap",
        font: "11px/1.5 ui-monospace, monospace",
        color: "#d4d4d4",
      },
    }),
  ];
}

async function lspHoverSafe(
  view: EditorView,
  ctx: LspContext,
  line: number,
  character: number,
): Promise<string | null> {
  try {
    return await lspHover({
      ...ctx,
      text: view.state.doc.toString(),
      line,
      character,
    });
  } catch (e) {
    if (!isUnsupported(e)) console.debug("lsp hover failed", e);
    return null;
  }
}

async function jumpToDefinition(
  view: EditorView,
  ctx: LspContext,
  line: number,
  character: number,
): Promise<void> {
  let locations: LspLocation[];
  try {
    locations = await lspDefinition({
      ...ctx,
      text: view.state.doc.toString(),
      line,
      character,
    });
  } catch (e) {
    if (!isUnsupported(e)) console.debug("lsp definition failed", e);
    return;
  }
  // Worktree-local targets sort first backend-side; anything outside the
  // worktree (crates.io sources, node_modules) can't be opened in a tab.
  const target = locations.find((l) => l.rel_path);
  if (target) ctx.openLocation(target);
}
