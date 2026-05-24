import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  File,
  FilePlus,
  Folder,
  FolderOpen,
  FolderPlus,
  Pencil,
  RefreshCw,
  Trash2,
} from "lucide-react";
import {
  PRIMARY_WORKTREE_ID,
  worktreeList,
  type WorktreeRow,
} from "~/ipc/worktree.ts";
import {
  joinPath,
  scopeKey,
  useFileEditorStore,
} from "~/stores/fileEditorStore.ts";
import type { FsEntry } from "~/ipc/fs.ts";

interface Props {
  projectId: string;
  worktreeId: string;
  onWorktreeChange: (id: string) => void;
}

// Stable empty references — selectors that return a fresh `{}` per render
// trigger zustand's default Object.is equality and cause re-render storms
// (and in React 19, the dev "too many renders" guard kills the whole tree).
const EMPTY_TREE: Record<string, never> = {};
const EMPTY_EXPANDED: Record<string, boolean> = {};

export function FileTreePanel({
  projectId,
  worktreeId,
  onWorktreeChange,
}: Props) {
  const { t } = useTranslation("files");
  const [worktrees, setWorktrees] = useState<WorktreeRow[]>([]);
  const key = scopeKey(projectId, worktreeId);
  const tree = useFileEditorStore((s) => s.trees[key] ?? EMPTY_TREE);
  const expanded = useFileEditorStore(
    (s) => s.expanded[key] ?? EMPTY_EXPANDED,
  );
  const loadDir = useFileEditorStore((s) => s.loadDir);
  const toggleExpand = useFileEditorStore((s) => s.toggleExpand);
  const openFile = useFileEditorStore((s) => s.openFile);
  const refreshDir = useFileEditorStore((s) => s.refreshDir);
  const createFile = useFileEditorStore((s) => s.createFile);
  const createDir = useFileEditorStore((s) => s.createDir);
  const renameEntry = useFileEditorStore((s) => s.renameEntry);
  const deleteEntry = useFileEditorStore((s) => s.deleteEntry);

  const [menu, setMenu] = useState<
    | { x: number; y: number; relPath: string; isDir: boolean }
    | null
  >(null);

  // Dismiss context menu on outside click / escape.
  useEffect(() => {
    if (!menu) return;
    const onDown = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenu(null);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  const promptNewFile = async (parentRel: string) => {
    setMenu(null);
    const name = window.prompt(t("prompt_new_file"));
    if (!name) return;
    const target = parentRel ? `${parentRel}/${name}` : name;
    try {
      await createFile(projectId, worktreeId, target);
      await openFile(projectId, worktreeId, target);
    } catch (e) {
      window.alert(`${t("op_failed")}: ${e instanceof Error ? e.message : e}`);
    }
  };

  const promptNewFolder = async (parentRel: string) => {
    setMenu(null);
    const name = window.prompt(t("prompt_new_folder"));
    if (!name) return;
    const target = parentRel ? `${parentRel}/${name}` : name;
    try {
      await createDir(projectId, worktreeId, target);
    } catch (e) {
      window.alert(`${t("op_failed")}: ${e instanceof Error ? e.message : e}`);
    }
  };

  const promptRename = async (relPath: string) => {
    setMenu(null);
    const idx = relPath.lastIndexOf("/");
    const oldName = idx >= 0 ? relPath.slice(idx + 1) : relPath;
    const parent = idx >= 0 ? relPath.slice(0, idx) : "";
    const newName = window.prompt(t("prompt_rename"), oldName);
    if (!newName || newName === oldName) return;
    const target = parent ? `${parent}/${newName}` : newName;
    try {
      await renameEntry(projectId, worktreeId, relPath, target);
    } catch (e) {
      window.alert(`${t("op_failed")}: ${e instanceof Error ? e.message : e}`);
    }
  };

  const confirmDelete = async (relPath: string, isDir: boolean) => {
    setMenu(null);
    if (!window.confirm(t("confirm_delete", { path: relPath }))) return;
    try {
      await deleteEntry(projectId, worktreeId, relPath, isDir);
    } catch (e) {
      window.alert(`${t("op_failed")}: ${e instanceof Error ? e.message : e}`);
    }
  };

  // Refresh worktrees when project changes.
  useEffect(() => {
    let cancelled = false;
    void worktreeList({ project_id: projectId }).then((rows) => {
      if (!cancelled) setWorktrees(rows);
    });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  // Load root the first time we see a worktree.
  useEffect(() => {
    if (!tree[""]) {
      void loadDir(projectId, worktreeId, "");
    }
  }, [projectId, worktreeId, tree, loadDir]);

  const rootChildren = tree[""]?.children ?? null;
  const rootLoading = tree[""]?.loading ?? false;
  const rootError = tree[""]?.error ?? null;

  return (
    <div className="flex h-full min-h-0 flex-col border-r border-neutral-800 bg-neutral-950 text-[12px]">
      <div className="flex items-center gap-1 border-b border-neutral-800 px-2 py-1.5">
        <select
          value={worktreeId}
          onChange={(e) => onWorktreeChange(e.target.value)}
          className="min-w-0 flex-1 rounded bg-neutral-900 px-1.5 py-0.5 text-[11px] text-neutral-200 outline-none focus:ring-1 focus:ring-neutral-700"
          aria-label={t("worktree_picker_label")}
        >
          {worktrees.map((w) => (
            <option key={w.id} value={w.id}>
              {w.name}
              {w.branch ? ` · ${w.branch}` : ""}
            </option>
          ))}
        </select>
        <button
          type="button"
          onClick={() => void refreshDir(projectId, worktreeId, "")}
          className="rounded p-1 text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
          title={t("refresh")}
          aria-label={t("refresh")}
        >
          <RefreshCw size={12} />
        </button>
      </div>

      <div
        className="min-h-0 flex-1 overflow-auto py-1"
        onContextMenu={(e) => {
          // Right-click on the empty area opens a root-scoped menu so the
          // user can create a new file/folder at the worktree root.
          if (e.target === e.currentTarget) {
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, relPath: "", isDir: true });
          }
        }}
      >
        {rootLoading && !rootChildren && (
          <div className="px-3 py-2 text-neutral-500">{t("loading")}</div>
        )}
        {rootError && (
          <div className="px-3 py-2 text-red-400" role="alert">
            {rootError}
          </div>
        )}
        {rootChildren && rootChildren.length === 0 && (
          <div className="px-3 py-2 text-neutral-500">{t("empty_dir")}</div>
        )}
        {rootChildren?.map((entry) => (
          <TreeNode
            key={entry.name}
            entry={entry}
            relPath={entry.name}
            depth={0}
            projectId={projectId}
            worktreeId={worktreeId}
            tree={tree}
            expanded={expanded}
            onToggle={toggleExpand}
            onOpen={openFile}
            onContextMenu={(x, y, relPath, isDir) =>
              setMenu({ x, y, relPath, isDir })
            }
          />
        ))}
      </div>

      {menu && (
        <div
          style={{ left: menu.x, top: menu.y }}
          className="fixed z-50 min-w-[160px] rounded border border-neutral-800 bg-neutral-950 py-1 text-[11px] shadow-lg"
          onMouseDown={(e) => e.stopPropagation()}
        >
          {menu.isDir && (
            <>
              <MenuItem
                icon={<FilePlus size={11} />}
                label={t("ctx_new_file")}
                onClick={() => void promptNewFile(menu.relPath)}
              />
              <MenuItem
                icon={<FolderPlus size={11} />}
                label={t("ctx_new_folder")}
                onClick={() => void promptNewFolder(menu.relPath)}
              />
            </>
          )}
          {menu.relPath !== "" && (
            <>
              {menu.isDir && (
                <div className="my-1 border-t border-neutral-800" />
              )}
              <MenuItem
                icon={<Pencil size={11} />}
                label={t("ctx_rename")}
                onClick={() => void promptRename(menu.relPath)}
              />
              <MenuItem
                icon={<Trash2 size={11} />}
                label={t("ctx_delete")}
                danger
                onClick={() => void confirmDelete(menu.relPath, menu.isDir)}
              />
            </>
          )}
        </div>
      )}
    </div>
  );
}

function MenuItem({
  icon,
  label,
  onClick,
  danger,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex w-full items-center gap-2 px-3 py-1 text-left ${
        danger
          ? "text-red-300 hover:bg-red-900/30"
          : "text-neutral-200 hover:bg-neutral-900"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

interface NodeProps {
  entry: FsEntry;
  relPath: string;
  depth: number;
  projectId: string;
  worktreeId: string;
  tree: Record<
    string,
    {
      relPath: string;
      children: FsEntry[] | null;
      loading: boolean;
      error: string | null;
    }
  >;
  expanded: Record<string, boolean>;
  onToggle: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  onOpen: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  onContextMenu: (x: number, y: number, relPath: string, isDir: boolean) => void;
}

function TreeNode({
  entry,
  relPath,
  depth,
  projectId,
  worktreeId,
  tree,
  expanded,
  onToggle,
  onOpen,
  onContextMenu,
}: NodeProps) {
  const isOpen = !!expanded[relPath];
  const dirNode = tree[relPath];
  const padding = useMemo(() => ({ paddingLeft: 8 + depth * 10 }), [depth]);

  if (!entry.is_dir) {
    return (
      <button
        type="button"
        onClick={() => void onOpen(projectId, worktreeId, relPath)}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onContextMenu(e.clientX, e.clientY, relPath, false);
        }}
        className="flex w-full items-center gap-1 px-2 py-0.5 text-left text-neutral-300 hover:bg-neutral-900"
        style={padding}
      >
        <File size={12} className="shrink-0 text-neutral-500" />
        <span className="truncate">{entry.name}</span>
      </button>
    );
  }

  return (
    <div>
      <button
        type="button"
        onClick={() => void onToggle(projectId, worktreeId, relPath)}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onContextMenu(e.clientX, e.clientY, relPath, true);
        }}
        className="flex w-full items-center gap-1 px-2 py-0.5 text-left text-neutral-200 hover:bg-neutral-900"
        style={padding}
      >
        {isOpen ? (
          <ChevronDown size={12} className="shrink-0 text-neutral-500" />
        ) : (
          <ChevronRight size={12} className="shrink-0 text-neutral-500" />
        )}
        {isOpen ? (
          <FolderOpen size={12} className="shrink-0 text-amber-300/80" />
        ) : (
          <Folder size={12} className="shrink-0 text-amber-300/80" />
        )}
        <span className="truncate">{entry.name}</span>
      </button>
      {isOpen && (
        <div>
          {dirNode?.loading && !dirNode.children && (
            <div
              className="px-2 py-0.5 text-neutral-500"
              style={{ paddingLeft: 8 + (depth + 1) * 10 }}
            >
              ...
            </div>
          )}
          {dirNode?.error && (
            <div
              className="px-2 py-0.5 text-red-400"
              style={{ paddingLeft: 8 + (depth + 1) * 10 }}
            >
              {dirNode.error}
            </div>
          )}
          {dirNode?.children?.map((child) => (
            <TreeNode
              key={child.name}
              entry={child}
              relPath={joinPath(relPath, child.name)}
              depth={depth + 1}
              projectId={projectId}
              worktreeId={worktreeId}
              tree={tree}
              expanded={expanded}
              onToggle={onToggle}
              onOpen={onOpen}
              onContextMenu={onContextMenu}
            />
          ))}
        </div>
      )}
    </div>
  );
}

// Re-export so consumers can import the constant alongside the component.
export { PRIMARY_WORKTREE_ID };
