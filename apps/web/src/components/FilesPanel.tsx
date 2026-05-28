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
import { useDragResize } from "~/lib/useDragResize.ts";

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

  const treeResize = useDragResize({
    storageKey: "oxyris.filesPanel.treeWidth",
    defaultSize: 288,
    min: 180,
    max: 640,
    axis: "horizontal",
    direction: "right",
  });

  const sessionWorktreeId = useMemo(() => {
    if (!sessionSnapshot) return null;
    // Only adopt the active session's worktree when that session actually
    // belongs to the project being viewed — otherwise the tree would try to
    // load another project's worktree under this project (root mismatch / the
    // wrong files showing). Fall back to PRIMARY for the selected project.
    if (sessionSnapshot.project_id !== projectId) return null;
    return sessionSnapshot.worktree_id ?? PRIMARY_WORKTREE_ID;
  }, [sessionSnapshot, projectId]);

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
      <div
        className="relative shrink-0"
        style={{ width: treeResize.size }}
      >
        <FileTreePanel
          projectId={projectId}
          worktreeId={worktreeId}
          onWorktreeChange={(id) =>
            setOverrides((prev) => ({ ...prev, [projectId]: id }))
          }
        />
        <div
          onMouseDown={treeResize.onResizeStart}
          role="separator"
          aria-orientation="vertical"
          className="group absolute right-0 top-0 z-10 h-full w-1 cursor-col-resize"
        >
          <div className="h-full w-full bg-transparent transition group-hover:bg-emerald-700/50" />
        </div>
      </div>
      <FileEditorTabs projectId={projectId} worktreeId={worktreeId} />
    </div>
  );
}
