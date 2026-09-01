import { create } from "zustand";

/**
 * Which worktree each panel that shows worktree-scoped content (Files, Git)
 * is pinned to, keyed by project id.
 *
 * Shared rather than per-panel local state so the panels agree: jumping from a
 * changed file in the Git panel into the editor must land in the same
 * `scopeKey(projectId, worktreeId)` the Files panel renders, otherwise the tab
 * opens into a scope nobody is looking at.
 *
 * In-memory only — an override lasts for the app session. With no override the
 * panels fall back to the active session's worktree, then PRIMARY.
 */
interface WorktreePickState {
  overrides: Record<string, string>;
  setOverride: (projectId: string, worktreeId: string) => void;
}

export const useWorktreePickStore = create<WorktreePickState>((set) => ({
  overrides: {},
  setOverride: (projectId, worktreeId) =>
    set((s) => ({ overrides: { ...s.overrides, [projectId]: worktreeId } })),
}));
