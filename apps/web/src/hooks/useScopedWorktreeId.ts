import { useMemo } from "react";
import { PRIMARY_WORKTREE_ID } from "~/ipc/worktree.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { useWorktreePickStore } from "~/stores/worktreePickStore.ts";

/**
 * The worktree every worktree-scoped surface of a project agrees on.
 *
 * Resolution order:
 * 1. The explicit per-project pick (the Files/Git worktree picker).
 * 2. The active session's worktree — but only when that session belongs to
 *    `projectId`; another project's worktree id under this project resolves to
 *    a root that isn't there.
 * 3. PRIMARY.
 *
 * Single source of truth on purpose: the Files panel, the Git panel and the
 * search dialogs (Find in Files, Search Everywhere, Quick Open) all key the
 * editor store by `scopeKey(projectId, worktreeId)`. When a dialog resolves a
 * different worktree than the panel renders, the tab it opens lands in a scope
 * nobody is looking at and the click looks like a no-op.
 */
export function useScopedWorktreeId(projectId: string | null): string {
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const sessionSnapshot = useSessionStore((s) =>
    activeSessionId ? s.snapshots[activeSessionId] : null,
  );
  const overrides = useWorktreePickStore((s) => s.overrides);

  return useMemo(() => {
    const sessionWorktreeId =
      sessionSnapshot && sessionSnapshot.project_id === projectId
        ? (sessionSnapshot.worktree_id ?? PRIMARY_WORKTREE_ID)
        : null;
    return (
      (projectId ? overrides[projectId] : null) ||
      sessionWorktreeId ||
      PRIMARY_WORKTREE_ID
    );
  }, [projectId, overrides, sessionSnapshot]);
}
