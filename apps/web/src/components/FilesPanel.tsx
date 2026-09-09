import { useTranslation } from "react-i18next";
import { useWorktreePickStore } from "~/stores/worktreePickStore.ts";
import { FileTreePanel } from "~/components/FileTreePanel.tsx";
import { FileEditorTabs } from "~/components/FileEditorTabs.tsx";
import { useDragResize } from "~/lib/useDragResize.ts";
import { useScopedWorktreeId } from "~/hooks/useScopedWorktreeId.ts";

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

  // Per-project explicit override of the active worktree. Shared with the Git
  // panel and the search dialogs — see useScopedWorktreeId.
  const setOverride = useWorktreePickStore((s) => s.setOverride);
  const worktreeId = useScopedWorktreeId(projectId);

  const treeResize = useDragResize({
    storageKey: "oxyris.filesPanel.treeWidth",
    defaultSize: 288,
    min: 180,
    max: 640,
    axis: "horizontal",
    direction: "right",
  });

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
          onWorktreeChange={(id) => setOverride(projectId, id)}
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
