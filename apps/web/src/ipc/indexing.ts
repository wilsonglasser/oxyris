import { invoke } from "@tauri-apps/api/core";

/**
 * Idempotent "make this worktree ready for code-aware tools": triggers an
 * initial tree-sitter index walk if the DB is empty + pre-warms the
 * primary language server. Safe to call repeatedly; backend fast-paths
 * when nothing needs to happen. Progress flows through `indexing:progress`
 * and `lsp:status` Tauri events the chips already listen to.
 */
export async function worktreeEnsureReady(input: {
  worktree_id: string;
  project_id?: string;
}): Promise<void> {
  await invoke("worktree_ensure_ready", { input });
}

export type SymbolKind =
  | "function"
  | "method"
  | "class"
  | "struct"
  | "enum"
  | "trait"
  | "interface"
  | "type"
  | "constant"
  | "module";

/** Symbol kinds shown under the "Classes" filter — type-like declarations. */
export const CLASS_LIKE_KINDS: SymbolKind[] = [
  "class",
  "struct",
  "enum",
  "trait",
  "interface",
  "type",
];

export type SymbolHit = {
  name: string;
  kind: SymbolKind;
  /** Path relative to the worktree root, forward slashes. */
  file: string;
  /** 1-based line numbers (inclusive). */
  start_line: number;
  start_col: number;
  end_line: number;
  end_col: number;
};

/** Query the per-worktree tree-sitter symbol index. Matches an exact name,
 *  falling back to a case-insensitive prefix. `projectId` is required when
 *  `worktreeId` is the primary sentinel. Returns `[]` when the worktree has
 *  no index yet (e.g. WSL projects mid-warmup) rather than throwing. */
export async function indexQuerySymbol(args: {
  worktreeId: string;
  projectId?: string;
  name: string;
  kind?: SymbolKind;
  limit?: number;
}): Promise<SymbolHit[]> {
  try {
    return await invoke<SymbolHit[]>("index_query_symbol", {
      input: {
        worktree_id: args.worktreeId,
        project_id: args.projectId,
        name: args.name,
        kind: args.kind,
        limit: args.limit ?? 30,
      },
    });
  } catch {
    return [];
  }
}
