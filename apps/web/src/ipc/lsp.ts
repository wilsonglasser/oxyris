import { invoke } from "@tauri-apps/api/core";

/**
 * Editor-facing LSP calls. Every one takes the *live buffer*: the backend
 * pushes it to the language server before answering, so results describe what
 * the user is looking at rather than what was last saved (and so WSL projects
 * work at all — the server's host can't read the distro's paths itself).
 *
 * `line` / `character` are 0-based, LSP-native. `character` counts UTF-16 code
 * units, which is what a JS string index already is, so mapping to CodeMirror
 * offsets is `doc.line(line + 1).from + character` with no conversion.
 */

export type LspSeverity = "error" | "warning" | "info" | "hint";

export type LspDiagnostic = {
  start_line: number;
  start_character: number;
  end_line: number;
  end_character: number;
  severity: LspSeverity;
  message: string;
  source: string | null;
};

export type LspLocation = {
  /** Set only when the target is inside the worktree — the editor can open it. */
  rel_path: string | null;
  abs_path: string;
  line: number;
  character: number;
};

export type LspTextEdit = {
  start_line: number;
  start_character: number;
  end_line: number;
  end_character: number;
  new_text: string;
};

type DocArgs = {
  projectId: string;
  worktreeId: string;
  relPath: string;
  text: string;
};

type PositionArgs = DocArgs & { line: number; character: number };

const doc = (a: DocArgs) => ({
  project_id: a.projectId,
  worktree_id: a.worktreeId,
  rel_path: a.relPath,
  text: a.text,
});

/**
 * True when the failure was "no language server covers this file" rather than
 * a real error. Callers use it to disable LSP for a tab instead of surfacing
 * anything — a `.txt` file having no server is not a problem to report.
 */
export function isUnsupported(e: unknown): boolean {
  return (
    !!e && typeof e === "object" && (e as { code?: string }).code === "unsupported"
  );
}

export function lspDiagnostics(args: DocArgs): Promise<LspDiagnostic[]> {
  return invoke<LspDiagnostic[]>("lsp_diagnostics", { input: doc(args) });
}

export function lspHover(args: PositionArgs): Promise<string | null> {
  return invoke<string | null>("lsp_hover", {
    input: { ...doc(args), line: args.line, character: args.character },
  });
}

export function lspDefinition(args: PositionArgs): Promise<LspLocation[]> {
  return invoke<LspLocation[]>("lsp_definition", {
    input: { ...doc(args), line: args.line, character: args.character },
  });
}

export function lspFormat(
  args: DocArgs & { tabSize: number; insertSpaces: boolean },
): Promise<LspTextEdit[]> {
  return invoke<LspTextEdit[]>("lsp_format", {
    input: {
      ...doc(args),
      tab_size: args.tabSize,
      insert_spaces: args.insertSpaces,
    },
  });
}

/** Save happened — lets the server run its check layer (`cargo check` &co.). */
export function lspDidSave(args: DocArgs): Promise<void> {
  return invoke<void>("lsp_did_save", { input: doc(args) });
}

/** Tab closed — the server drops our buffer and trusts disk again. */
export function lspDidClose(args: {
  projectId: string;
  worktreeId: string;
  relPath: string;
}): Promise<void> {
  return invoke<void>("lsp_did_close", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      rel_path: args.relPath,
    },
  });
}
