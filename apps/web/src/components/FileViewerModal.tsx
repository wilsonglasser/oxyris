import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { EditorPane } from "~/components/FileEditorTabs.tsx";
import { scopeKey, useFileEditorStore } from "~/stores/fileEditorStore.ts";

interface Props {
  projectId: string;
  worktreeId: string;
  /** Path relative to the worktree root. */
  relPath: string;
  onClose: () => void;
}

/**
 * In-app file viewer/editor in a modal overlay. Reuses the same store-backed
 * {@link EditorPane} as the Files tab — so a file opened here also appears as a
 * tab there, and edits/saves share one buffer. Triggered by Ctrl/Cmd+click on a
 * path in a terminal (see {@link TerminalView}).
 */
export function FileViewerModal({ projectId, worktreeId, relPath, onClose }: Props) {
  const { t } = useTranslation("files");
  const openFile = useFileEditorStore((s) => s.openFile);
  const tab = useFileEditorStore(
    (s) => s.tabs[scopeKey(projectId, worktreeId)]?.[relPath] ?? null,
  );

  // Load (or focus) the file in the shared store when the target changes.
  useEffect(() => {
    void openFile(projectId, worktreeId, relPath);
  }, [openFile, projectId, worktreeId, relPath]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="relative flex h-[80vh] w-full max-w-5xl flex-col overflow-hidden rounded-xl border border-neutral-800 bg-neutral-950 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex h-9 shrink-0 items-center justify-between border-b border-neutral-800 bg-neutral-900 pl-3 pr-2 text-[12px] text-neutral-300">
          <span className="truncate font-mono" title={relPath}>
            {relPath}
          </span>
          <button
            type="button"
            aria-label={t("close_tab")}
            onClick={onClose}
            className="flex size-6 items-center justify-center rounded text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
          >
            <X size={14} />
          </button>
        </header>
        <div className="flex min-h-0 flex-1 flex-col">
          {tab ? (
            <EditorPane
              key={`${worktreeId}::${relPath}::${tab.loading ? "load" : "ready"}`}
              projectId={projectId}
              worktreeId={worktreeId}
              tab={tab}
            />
          ) : (
            <div className="flex flex-1 items-center justify-center text-[12px] text-neutral-500">
              {t("loading")}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
