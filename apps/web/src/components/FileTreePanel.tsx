import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  ClipboardPaste,
  Copy,
  ExternalLink,
  File,
  FileDiff,
  FilePlus,
  Folder,
  FolderOpen,
  FolderPlus,
  GitBranch,
  GitCommit,
  History,
  Link2,
  Pencil,
  RefreshCw,
  Scissors,
  Search,
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
import { fsAbsPath, fsOpenExternal, fsReveal, type FsEntry } from "~/ipc/fs.ts";
import {
  buildHighlightRegex,
  highlightMatches,
} from "~/lib/searchHighlight.tsx";
import { RevDiffModal } from "~/components/RevDiffModal.tsx";
import {
  CompareRefModal,
  FileHistoryModal,
} from "~/components/FileGitCompare.tsx";
import { MenuSurface } from "~/components/MenuSurface.tsx";

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
  const clipboard = useFileEditorStore((s) => s.clipboard);
  const setClipboard = useFileEditorStore((s) => s.setClipboard);
  const pasteInto = useFileEditorStore((s) => s.pasteInto);

  // Tree selection (single click) + JetBrains-style speed search. `selected`
  // is the highlighted node; `speedQuery` filters/highlights the *currently
  // visible* nodes only (we never auto-expand the whole tree to search).
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [speedQuery, setSpeedQuery] = useState("");

  const [menu, setMenu] = useState<
    | { x: number; y: number; relPath: string; isDir: boolean }
    | null
  >(null);
  // Single-file git diff (Show diff / Compare-with picks land here).
  const [revDiff, setRevDiff] = useState<
    { from: string; to: string; title: string; pathFilter: string } | null
  >(null);
  // Branch/tag or revision picker for the "Compare with…" actions.
  const [comparePicker, setComparePicker] = useState<
    { kind: "refs" | "revs"; relPath: string } | null
  >(null);
  // File history modal.
  const [history, setHistory] = useState<{ relPath: string } | null>(null);

  const parentOf = (relPath: string) => {
    const idx = relPath.lastIndexOf("/");
    return idx >= 0 ? relPath.slice(0, idx) : "";
  };

  const opFailed = (e: unknown) =>
    window.alert(`${t("op_failed")}: ${e instanceof Error ? e.message : e}`);

  const doOpenExternal = async (relPath: string) => {
    setMenu(null);
    try {
      await fsOpenExternal({ projectId, worktreeId, relPath });
    } catch (e) {
      opFailed(e);
    }
  };

  const doReveal = async (relPath: string) => {
    setMenu(null);
    try {
      await fsReveal({ projectId, worktreeId, relPath });
    } catch (e) {
      opFailed(e);
    }
  };

  const copyPath = async (relPath: string, relative: boolean) => {
    setMenu(null);
    try {
      const text = relative
        ? relPath
        : await fsAbsPath({ projectId, worktreeId, relPath });
      await navigator.clipboard.writeText(text);
    } catch (e) {
      opFailed(e);
    }
  };

  const doPaste = async (destDir: string) => {
    setMenu(null);
    try {
      await pasteInto(projectId, worktreeId, destDir);
    } catch (e) {
      opFailed(e);
    }
  };

  const showDiff = (relPath: string) => {
    setMenu(null);
    setRevDiff({
      from: "HEAD",
      to: "WORKTREE",
      title: t("file_show_diff_title", { path: relPath }),
      pathFilter: relPath,
    });
  };

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

  // Flattened list of nodes the tree is *currently rendering* (root +
  // expanded descendants). Drives keyboard nav + speed search.
  const flatVisible = useMemo(
    () => flattenVisible(rootChildren, tree, expanded),
    [rootChildren, tree, expanded],
  );

  const speedRe = useMemo(
    () => buildHighlightRegex(speedQuery),
    [speedQuery],
  );
  const speedMatches = useMemo(() => {
    if (!speedQuery) return flatVisible;
    const q = speedQuery.toLowerCase();
    return flatVisible.filter((n) => n.name.toLowerCase().includes(q));
  }, [flatVisible, speedQuery]);

  // Jump selection to the first speed-search match as the user types.
  useEffect(() => {
    if (!speedQuery) return;
    const first = speedMatches[0];
    if (first) setSelected(first.relPath);
  }, [speedQuery, speedMatches]);

  // Keep the selected row scrolled into view.
  useEffect(() => {
    if (!selected) return;
    const el = containerRef.current?.querySelector<HTMLElement>(
      `[data-tree-rel="${cssEscape(selected)}"]`,
    );
    el?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const selectNode = useCallback((relPath: string) => {
    setSelected(relPath);
    containerRef.current?.focus();
  }, []);

  const activate = useCallback(
    (relPath: string) => {
      const node = flatVisible.find((n) => n.relPath === relPath);
      if (!node) return;
      if (node.isDir) void toggleExpand(projectId, worktreeId, relPath);
      else void openFile(projectId, worktreeId, relPath);
    },
    [flatVisible, toggleExpand, openFile, projectId, worktreeId],
  );

  const moveSelection = (dir: 1 | -1) => {
    const list = speedQuery ? speedMatches : flatVisible;
    if (list.length === 0) return;
    const idx = list.findIndex((n) => n.relPath === selected);
    const next =
      idx < 0
        ? dir > 0
          ? 0
          : list.length - 1
        : Math.min(Math.max(idx + dir, 0), list.length - 1);
    setSelected(list[next]!.relPath);
  };

  const onTreeKeyDown = (e: React.KeyboardEvent) => {
    // Let global editor shortcuts (Ctrl+N / Ctrl+Shift+F / …) through.
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      moveSelection(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      moveSelection(-1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (selected) activate(selected);
    } else if (e.key === "ArrowRight") {
      const n = flatVisible.find((x) => x.relPath === selected);
      if (selected && n?.isDir && !expanded[selected]) {
        e.preventDefault();
        void toggleExpand(projectId, worktreeId, selected);
      }
    } else if (e.key === "ArrowLeft") {
      const n = flatVisible.find((x) => x.relPath === selected);
      if (selected && n?.isDir && expanded[selected]) {
        e.preventDefault();
        void toggleExpand(projectId, worktreeId, selected);
      }
    } else if (e.key === "Escape") {
      if (speedQuery) {
        e.preventDefault();
        setSpeedQuery("");
      }
    } else if (e.key === "Backspace") {
      if (speedQuery) {
        e.preventDefault();
        setSpeedQuery((q) => q.slice(0, -1));
      }
    } else if (e.key.length === 1) {
      // Printable char → start / extend speed search.
      e.preventDefault();
      setSpeedQuery((q) => q + e.key);
    }
  };

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
        ref={containerRef}
        tabIndex={0}
        role="tree"
        className="relative min-h-0 flex-1 overflow-auto py-1 outline-none"
        onKeyDown={onTreeKeyDown}
        onBlur={(e) => {
          // Drop the speed-search filter when focus leaves the tree entirely.
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
            setSpeedQuery("");
          }
        }}
        onContextMenu={(e) => {
          // Right-click on the empty area opens a root-scoped menu so the
          // user can create a new file/folder at the worktree root.
          if (e.target === e.currentTarget) {
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, relPath: "", isDir: true });
          }
        }}
      >
        {speedQuery && (
          <div className="sticky top-0 z-20 mx-1 mb-1 flex items-center gap-1.5 rounded border border-neutral-700 bg-neutral-900 px-2 py-1 text-[11px] shadow">
            <Search size={11} className="text-neutral-500" />
            <span className="text-neutral-100">{speedQuery}</span>
            <span className="ml-auto text-neutral-500">
              {speedMatches.length > 0
                ? `${speedMatches.length}`
                : t("speed_search_no_match")}
            </span>
          </div>
        )}
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
            selected={selected}
            highlightRe={speedRe}
            onToggle={toggleExpand}
            onOpen={openFile}
            onSelect={selectNode}
            onContextMenu={(x, y, relPath, isDir) => {
              setSelected(relPath || null);
              setMenu({ x, y, relPath, isDir });
            }}
          />
        ))}
      </div>

      {menu && (
        <MenuSurface x={menu.x} y={menu.y} className="min-w-[200px]">
          {/* Open actions — files only. */}
          {menu.relPath !== "" && !menu.isDir && (
            <>
              <MenuItem
                icon={<File size={11} />}
                label={t("ctx_open")}
                onClick={() => {
                  void openFile(projectId, worktreeId, menu.relPath);
                  setMenu(null);
                }}
              />
              <MenuItem
                icon={<ExternalLink size={11} />}
                label={t("ctx_open_external")}
                onClick={() => void doOpenExternal(menu.relPath)}
              />
            </>
          )}
          {menu.relPath !== "" && (
            <MenuItem
              icon={<FolderOpen size={11} />}
              label={t("ctx_reveal")}
              onClick={() => void doReveal(menu.relPath)}
            />
          )}

          {/* Git actions — files only. */}
          {menu.relPath !== "" && !menu.isDir && (
            <>
              <Separator />
              <MenuItem
                icon={<FileDiff size={11} />}
                label={t("ctx_show_diff")}
                onClick={() => showDiff(menu.relPath)}
              />
              <MenuItem
                icon={<GitBranch size={11} />}
                label={t("ctx_compare_branch")}
                onClick={() => {
                  setComparePicker({ kind: "refs", relPath: menu.relPath });
                  setMenu(null);
                }}
              />
              <MenuItem
                icon={<GitCommit size={11} />}
                label={t("ctx_compare_rev")}
                onClick={() => {
                  setComparePicker({ kind: "revs", relPath: menu.relPath });
                  setMenu(null);
                }}
              />
              <MenuItem
                icon={<History size={11} />}
                label={t("ctx_show_history")}
                onClick={() => {
                  setHistory({ relPath: menu.relPath });
                  setMenu(null);
                }}
              />
            </>
          )}

          {/* Clipboard. */}
          <Separator />
          {menu.relPath !== "" && (
            <>
              <MenuItem
                icon={<Copy size={11} />}
                label={t("ctx_copy")}
                onClick={() => {
                  setClipboard({
                    projectId,
                    worktreeId,
                    relPath: menu.relPath,
                    op: "copy",
                  });
                  setMenu(null);
                }}
              />
              <MenuItem
                icon={<Scissors size={11} />}
                label={t("ctx_cut")}
                onClick={() => {
                  setClipboard({
                    projectId,
                    worktreeId,
                    relPath: menu.relPath,
                    op: "cut",
                  });
                  setMenu(null);
                }}
              />
            </>
          )}
          <MenuItem
            icon={<ClipboardPaste size={11} />}
            label={t("ctx_paste")}
            disabled={!clipboard}
            onClick={() =>
              void doPaste(menu.isDir ? menu.relPath : parentOf(menu.relPath))
            }
          />
          {menu.relPath !== "" && (
            <>
              <MenuItem
                icon={<Link2 size={11} />}
                label={t("ctx_copy_path")}
                onClick={() => void copyPath(menu.relPath, false)}
              />
              <MenuItem
                icon={<Link2 size={11} />}
                label={t("ctx_copy_rel_path")}
                onClick={() => void copyPath(menu.relPath, true)}
              />
            </>
          )}

          {/* Create — dirs (and root) only. */}
          {menu.isDir && (
            <>
              <Separator />
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

          {/* Rename / delete — any non-root entry. */}
          {menu.relPath !== "" && (
            <>
              <Separator />
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
        </MenuSurface>
      )}

      {revDiff && (
        <RevDiffModal
          projectId={projectId}
          worktreeId={worktreeId}
          from={revDiff.from}
          to={revDiff.to}
          title={revDiff.title}
          pathFilter={revDiff.pathFilter}
          open
          onClose={() => setRevDiff(null)}
        />
      )}
      {comparePicker && (
        <CompareRefModal
          projectId={projectId}
          worktreeId={worktreeId}
          kind={comparePicker.kind}
          relPath={comparePicker.relPath}
          onPick={(ref, label) => {
            const rel = comparePicker.relPath;
            setComparePicker(null);
            setRevDiff({
              from: ref,
              to: "WORKTREE",
              title: t("file_compare_title", { path: rel, ref: label }),
              pathFilter: rel,
            });
          }}
          onClose={() => setComparePicker(null)}
        />
      )}
      {history && (
        <FileHistoryModal
          projectId={projectId}
          worktreeId={worktreeId}
          relPath={history.relPath}
          onClose={() => setHistory(null)}
        />
      )}
    </div>
  );
}

function Separator() {
  return <div className="my-1 border-t border-neutral-800" />;
}

function MenuItem({
  icon,
  label,
  onClick,
  danger,
  disabled,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  danger?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={`flex w-full items-center gap-2 px-3 py-1 text-left disabled:cursor-default disabled:opacity-40 ${
        danger
          ? "text-red-300 enabled:hover:bg-red-900/30"
          : "text-neutral-200 enabled:hover:bg-neutral-900"
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
  selected: string | null;
  highlightRe: RegExp | null;
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
  onSelect: (relPath: string) => void;
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
  selected,
  highlightRe,
  onToggle,
  onOpen,
  onSelect,
  onContextMenu,
}: NodeProps) {
  const isOpen = !!expanded[relPath];
  const dirNode = tree[relPath];
  const padding = useMemo(() => ({ paddingLeft: 8 + depth * 10 }), [depth]);
  const isSelected = selected === relPath;

  // Single click = select; double click = open file / toggle folder. The
  // chevron toggles a folder on its own single click so the tree is still
  // explorable without double-clicking.
  const rowClass = `flex w-full cursor-default items-center gap-1 px-2 py-0.5 text-left ${
    isSelected
      ? "bg-sky-500/20 text-neutral-100"
      : "text-neutral-300 hover:bg-neutral-900"
  }`;
  const label = <span className="truncate">{highlightMatches(entry.name, highlightRe)}</span>;

  if (!entry.is_dir) {
    return (
      <div
        role="treeitem"
        data-tree-rel={relPath}
        onClick={() => onSelect(relPath)}
        onDoubleClick={() => void onOpen(projectId, worktreeId, relPath)}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onContextMenu(e.clientX, e.clientY, relPath, false);
        }}
        className={rowClass}
        style={padding}
      >
        <span className="w-3 shrink-0" />
        <File size={12} className="shrink-0 text-neutral-500" />
        {label}
      </div>
    );
  }

  return (
    <div>
      <div
        role="treeitem"
        aria-expanded={isOpen}
        data-tree-rel={relPath}
        onClick={() => onSelect(relPath)}
        onDoubleClick={() => void onToggle(projectId, worktreeId, relPath)}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          onContextMenu(e.clientX, e.clientY, relPath, true);
        }}
        className={rowClass}
        style={padding}
      >
        <button
          type="button"
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            void onToggle(projectId, worktreeId, relPath);
          }}
          className="shrink-0 text-neutral-500"
          aria-hidden
        >
          {isOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        </button>
        {isOpen ? (
          <FolderOpen size={12} className="shrink-0 text-amber-300/80" />
        ) : (
          <Folder size={12} className="shrink-0 text-amber-300/80" />
        )}
        {label}
      </div>
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
              selected={selected}
              highlightRe={highlightRe}
              onToggle={onToggle}
              onOpen={onOpen}
              onSelect={onSelect}
              onContextMenu={onContextMenu}
            />
          ))}
        </div>
      )}
    </div>
  );
}

// ────── visible-node flattening for keyboard nav + speed search ────────────

type FlatNode = {
  relPath: string;
  name: string;
  isDir: boolean;
};

function flattenVisible(
  rootChildren: FsEntry[] | null,
  tree: Record<string, { children: FsEntry[] | null }>,
  expanded: Record<string, boolean>,
): FlatNode[] {
  const out: FlatNode[] = [];
  const walk = (entries: FsEntry[], parentRel: string) => {
    for (const e of entries) {
      const rel = parentRel ? `${parentRel}/${e.name}` : e.name;
      out.push({ relPath: rel, name: e.name, isDir: e.is_dir });
      if (e.is_dir && expanded[rel]) {
        const kids = tree[rel]?.children;
        if (kids) walk(kids, rel);
      }
    }
  };
  if (rootChildren) walk(rootChildren, "");
  return out;
}

/** Polyfill for CSS.escape so arbitrary relPaths are safe in selectors. */
function cssEscape(s: string): string {
  if (typeof CSS !== "undefined" && CSS.escape) return CSS.escape(s);
  return s.replace(/["\\]/g, "\\$&");
}

// Re-export so consumers can import the constant alongside the component.
export { PRIMARY_WORKTREE_ID };
