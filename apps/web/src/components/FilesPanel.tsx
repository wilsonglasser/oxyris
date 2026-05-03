import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  PRIMARY_WORKTREE_ID,
  worktreeList,
  type WorktreeRow,
} from "~/ipc/worktree.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { FileTreePanel } from "~/components/FileTreePanel.tsx";
import { FileEditorTabs } from "~/components/FileEditorTabs.tsx";

interface Props {
  projectId: string | null;
}

/**
 * Files panel: tree on the left, editor tabs on the right.
 *
 * Worktree selection logic:
 * 1. If there's an active session, scope to its worktree (or PRIMARY).
 * 2. Otherwise, default to PRIMARY for the active project.
 * 3. The tree's worktree picker can override (1) and (2) per project.
 */
export function FilesPanel({ projectId }: Props) {
  const { t } = useTranslation("files");
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const sessionSnapshot = useSessionStore((s) =>
    activeSessionId ? s.snapshots[activeSessionId] : null,
  );

  // Per-project explicit override of the active worktree. Persists in-memory
  // for the session so switching tabs doesn't lose the user's pick.
  const [overrides, setOverrides] = useState<Record<string, string>>({});

  const sessionWorktreeId = useMemo(() => {
    if (!sessionSnapshot) return null;
    return sessionSnapshot.worktree_id ?? PRIMARY_WORKTREE_ID;
  }, [sessionSnapshot]);

  const worktreeId =
    (projectId && overrides[projectId]) ||
    sessionWorktreeId ||
    PRIMARY_WORKTREE_ID;

  if (!projectId) {
    return (
      <div className="flex h-full flex-1 items-center justify-center text-[12px] text-neutral-500">
        {t("pick_project")}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-1">
      <div className="w-72 shrink-0">
        <FileTreePanel
          projectId={projectId}
          worktreeId={worktreeId}
          onWorktreeChange={(id) =>
            setOverrides((prev) => ({ ...prev, [projectId]: id }))
          }
        />
      </div>
      <FileEditorTabs projectId={projectId} worktreeId={worktreeId} />
    </div>
  );
}
