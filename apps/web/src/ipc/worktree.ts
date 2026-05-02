import { invoke } from "@tauri-apps/api/core";

export type WorktreeRow = {
  id: string;
  project_id: string;
  name: string;
  branch: string;
  path: string;
  is_primary: boolean;
  created_at: string;
  removed_at: string | null;
};

export type BranchInfo = {
  name: string;
  is_current: boolean;
  is_remote: boolean;
};

export type WorktreeError =
  | { code: "domain"; message: string }
  | { code: "git"; message: string }
  | { code: "storage"; message: string }
  | { code: "project_not_found" }
  | { code: "projection"; message: string }
  | { code: "empty_repo" };

/**
 * Tauri rejects commands by returning the serialized {@link TauriWorktreeError}
 * (see `apps/desktop/src/tauri_commands/worktree.rs`). It arrives as a plain
 * object — wrapping it as an `Error` keeps callers from rendering
 * `[object Object]`.
 */
export class WorktreeCommandError extends Error {
  readonly tauri: WorktreeError;
  constructor(tauri: WorktreeError) {
    super(
      tauri.code === "project_not_found"
        ? "project not found"
        : tauri.code === "empty_repo"
          ? "repository has no commits yet"
          : `${tauri.code}: ${tauri.message}`,
    );
    this.tauri = tauri;
    this.name = "WorktreeCommandError";
  }
}

function wrapError(unknown: unknown): never {
  if (
    unknown &&
    typeof unknown === "object" &&
    "code" in unknown &&
    typeof (unknown as { code: unknown }).code === "string"
  ) {
    throw new WorktreeCommandError(unknown as WorktreeError);
  }
  throw unknown;
}

export async function worktreeList(input: {
  project_id: string;
  include_removed?: boolean;
}): Promise<WorktreeRow[]> {
  try {
    return await invoke<WorktreeRow[]>("worktree_list", { input });
  } catch (err) {
    wrapError(err);
  }
}

export async function worktreeCreate(input: {
  project_id: string;
  branch: string;
  name?: string;
}): Promise<WorktreeRow> {
  try {
    return await invoke<WorktreeRow>("worktree_create", { input });
  } catch (err) {
    wrapError(err);
  }
}

export async function worktreeRemove(input: { id: string }): Promise<void> {
  try {
    await invoke<void>("worktree_remove", { input });
  } catch (err) {
    wrapError(err);
  }
}

export async function gitListBranches(projectId: string): Promise<BranchInfo[]> {
  try {
    return await invoke<BranchInfo[]>("git_list_branches", { projectId });
  } catch (err) {
    wrapError(err);
  }
}
