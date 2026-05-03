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
