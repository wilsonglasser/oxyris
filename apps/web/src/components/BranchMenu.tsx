import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowDownToLine,
  ArrowUpToLine,
  Check,
  ChevronDown,
  ChevronRight,
  Cloud,
  Download,
  FileDiff as FileDiffIcon,
  Folder,
  GitBranch,
  GitMerge,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import type { BranchDetail } from "~/ipc/git.ts";
import { useGitStore } from "~/stores/gitStore.ts";
import { useAnchoredMenu } from "~/hooks/useAnchoredMenu.ts";

const EMPTY: BranchDetail[] = [];

interface Props {
  projectId: string;
  worktreeId: string;
  /** Raise a two-rev comparison to the panel, which owns the diff modal. */
  onCompare: (from: string, to: string, title: string) => void;
  onClose: () => void;
}

/**
 * Branch manager popup, modelled on the JetBrains branch widget: a search box
 * that filters both actions and branches, the repo-wide actions on top, then
 * the branch tree grouped by `/` segments. Each branch opens a flyout with the
 * operations that make sense for it (merge, rebase, compare, rename, delete).
 */
export function BranchMenu({ projectId, worktreeId, onCompare, onClose }: Props) {
  const { t } = useTranslation("git");
  const branches = useGitStore((s) => s.branchDetails[worktreeId] ?? EMPTY);
  const loading = useGitStore((s) => s.branchesLoading[worktreeId] ?? false);
  const running = useGitStore((s) => s.remote[worktreeId]?.running ?? false);
  const refresh = useGitStore((s) => s.refreshBranchDetails);
  const checkout = useGitStore((s) => s.checkout);
  const checkoutRemote = useGitStore((s) => s.checkoutRemoteBranch);
  const createBranch = useGitStore((s) => s.createBranch);
  const updateProject = useGitStore((s) => s.updateProject);
  const fetch = useGitStore((s) => s.fetch);
  const push = useGitStore((s) => s.push);

  const [filter, setFilter] = useState("");
  const [flyout, setFlyout] = useState<{
    branch: BranchDetail;
    x: number;
    y: number;
  } | null>(null);
  const [pending, setPending] = useState<PendingName | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    void refresh(projectId, worktreeId);
  }, [projectId, worktreeId, refresh]);

  // Click-outside + Escape close. Mousedown (not click) so it fires before a
  // re-render can detach the target element.
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (flyout) setFlyout(null);
        else if (pending) setPending(null);
        else onClose();
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose, flyout, pending]);

  const current = branches.find((b) => b.is_current) ?? null;
  const q = filter.trim().toLowerCase();

  const actions: MenuAction[] = useMemo(
    () => [
      {
        key: "update",
        label: t("update_project"),
        icon: <Download size={11} />,
        run: () => void updateProject(projectId, worktreeId),
      },
      {
        key: "fetch",
        label: t("fetch"),
        icon: <RefreshCw size={11} />,
        run: () => void fetch(projectId, worktreeId),
      },
      {
        key: "push",
        label: t("push"),
        icon: <ArrowUpToLine size={11} />,
        run: () => void push(projectId, worktreeId, true),
      },
      {
        key: "new_branch",
        label: t("new_branch"),
        icon: <Plus size={11} />,
        // Keeps the popup open: the name is typed into the inline row below.
        run: () => setPending({ kind: "new_branch" }),
        keepOpen: true,
      },
      {
        key: "checkout_rev",
        label: t("checkout_rev"),
        icon: <GitBranch size={11} />,
        run: () => setPending({ kind: "checkout_rev" }),
        keepOpen: true,
      },
    ],
    [t, projectId, worktreeId, updateProject, fetch, push],
  );

  const shownActions = actions.filter((a) => a.label.toLowerCase().includes(q));
  const locals = branches.filter((b) => !b.is_remote && matches(b, q));
  const remotes = branches.filter((b) => b.is_remote && matches(b, q));

  const runAction = (a: MenuAction) => {
    a.run();
    if (!a.keepOpen) onClose();
  };

  const onPickBranch = (b: BranchDetail) => {
    if (b.is_current) return;
    onClose();
    if (b.is_remote) {
      void checkoutRemote(projectId, worktreeId, b.name);
    } else {
      void checkout(projectId, worktreeId, b.name);
    }
  };

  const submitName = (value: string) => {
    const name = value.trim();
    setPending(null);
    if (!name || !pending) return;
    onClose();
    switch (pending.kind) {
      case "new_branch":
        void createBranch(projectId, worktreeId, name, true, pending.from);
        break;
      case "checkout_rev":
        void checkout(projectId, worktreeId, name);
        break;
      case "rename":
        void useGitStore
          .getState()
          .renameBranch(projectId, worktreeId, pending.branch, name);
        break;
    }
  };

  return (
    <div
      ref={rootRef}
      className="absolute left-0 top-full z-30 mt-1 flex max-h-[70vh] w-[340px] flex-col overflow-hidden rounded border border-neutral-800 bg-neutral-950 shadow-xl"
    >
      <div className="flex items-center gap-1 border-b border-neutral-800 px-2 py-1.5">
        <input
          type="text"
          autoFocus
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            if (shownActions.length === 1 && locals.length + remotes.length === 0) {
              runAction(shownActions[0]!);
            } else {
              const first = locals[0] ?? remotes[0];
              if (first) onPickBranch(first);
            }
          }}
          placeholder={t("branch_menu_search")}
          className="min-w-0 flex-1 rounded bg-neutral-900 px-2 py-1 text-[11px] text-neutral-200 outline-none focus:ring-1 focus:ring-neutral-700"
        />
        {(loading || running) && (
          <Loader2 size={12} className="animate-spin text-neutral-500" />
        )}
      </div>

      {pending && (
        <NameRow
          label={
            pending.kind === "rename"
              ? t("rename_branch_to", { name: pending.branch })
              : pending.kind === "checkout_rev"
                ? t("checkout_rev_placeholder")
                : pending.from
                  ? t("new_branch_from", { name: pending.from })
                  : t("new_branch_placeholder")
          }
          initial={pending.kind === "rename" ? pending.branch : ""}
          onSubmit={submitName}
          onCancel={() => setPending(null)}
        />
      )}

      <div className="min-h-0 flex-1 overflow-auto py-1">
        {shownActions.map((a) => (
          <button
            key={a.key}
            type="button"
            onClick={() => runAction(a)}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-[11px] text-neutral-200 hover:bg-neutral-900"
          >
            <span className="text-neutral-500">{a.icon}</span>
            {a.label}
          </button>
        ))}

        {current && q.length === 0 && (
          <>
            <Divider />
            <div className="px-3 pb-0.5 pt-1 text-[10px] uppercase tracking-wide text-neutral-500">
              {t("current_branch")}
            </div>
            <BranchRow
              branch={current}
              depth={0}
              onPick={() => undefined}
              onFlyout={(rect) =>
                setFlyout({ branch: current, x: rect.left, y: rect.bottom })
              }
            />
          </>
        )}

        {locals.length > 0 && (
          <>
            <Divider />
            <GroupHeader
              icon={<GitBranch size={11} />}
              label={t("local_branches", { count: locals.length })}
            />
            <BranchTree
              branches={locals.filter((b) => !b.is_current || q.length > 0)}
              flat={q.length > 0}
              onPick={onPickBranch}
              onFlyout={(b, rect) =>
                setFlyout({ branch: b, x: rect.left, y: rect.bottom })
              }
            />
          </>
        )}

        {remotes.length > 0 && (
          <>
            <Divider />
            <GroupHeader
              icon={<Cloud size={11} />}
              label={t("remote_branches", { count: remotes.length })}
            />
            <BranchTree
              branches={remotes}
              flat={q.length > 0}
              onPick={onPickBranch}
              onFlyout={(b, rect) =>
                setFlyout({ branch: b, x: rect.left, y: rect.bottom })
              }
            />
          </>
        )}

        {!loading && branches.length === 0 && (
          <div className="px-3 py-2 text-[11px] text-neutral-500">
            {t("no_branches")}
          </div>
        )}
      </div>

      {flyout && (
        <BranchActions
          projectId={projectId}
          worktreeId={worktreeId}
          branch={flyout.branch}
          current={current}
          x={flyout.x}
          y={flyout.y}
          onCompare={onCompare}
          onRename={(name) => {
            setFlyout(null);
            setPending({ kind: "rename", branch: name });
          }}
          onNewBranchFrom={(name) => {
            setFlyout(null);
            setPending({ kind: "new_branch", from: name });
          }}
          onDone={() => {
            setFlyout(null);
            onClose();
          }}
          onClose={() => setFlyout(null)}
        />
      )}
    </div>
  );
}

type PendingName =
  | { kind: "new_branch"; from?: string }
  | { kind: "checkout_rev" }
  | { kind: "rename"; branch: string };

interface MenuAction {
  key: string;
  label: string;
  icon: React.ReactNode;
  run: () => void;
  /** Actions that open an inline input keep the popup mounted. */
  keepOpen?: boolean;
}

function matches(b: BranchDetail, q: string): boolean {
  return q.length === 0 || b.name.toLowerCase().includes(q);
}

function Divider() {
  return <div className="my-1 border-t border-neutral-800/70" />;
}

function GroupHeader({
  icon,
  label,
}: {
  icon: React.ReactNode;
  label: string;
}) {
  return (
    <div className="flex items-center gap-1 px-3 pb-0.5 pt-1 text-[10px] uppercase tracking-wide text-neutral-500">
      {icon}
      {label}
    </div>
  );
}

/**
 * Inline name prompt. `window.prompt` is a no-op in WebView2, so every flow
 * that needs a string uses this row instead.
 */
function NameRow({
  label,
  initial,
  onSubmit,
  onCancel,
}: {
  label: string;
  initial: string;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <div className="border-b border-neutral-800 bg-neutral-900/60 px-2 py-1.5">
      <div className="mb-1 text-[10px] text-neutral-400">{label}</div>
      <input
        type="text"
        autoFocus
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") onSubmit(value);
          else if (e.key === "Escape") {
            e.stopPropagation();
            onCancel();
          }
        }}
        className="w-full rounded bg-neutral-950 px-2 py-1 text-[11px] text-neutral-100 outline-none ring-1 ring-neutral-700 focus:ring-emerald-700"
      />
    </div>
  );
}

// ────── branch tree ────────────────────────────────────────────────────────

type TreeNode =
  | { kind: "leaf"; branch: BranchDetail }
  | { kind: "folder"; label: string; children: TreeNode[] };

/** Group branch names on `/` so `feature/a` + `feature/b` nest under `feature`. */
function buildTree(branches: BranchDetail[], depth: number): TreeNode[] {
  const folders = new Map<string, BranchDetail[]>();
  const leaves: BranchDetail[] = [];
  for (const b of branches) {
    const parts = b.name.split("/");
    if (parts.length > depth + 1) {
      const key = parts[depth]!;
      const bucket = folders.get(key);
      if (bucket) bucket.push(b);
      else folders.set(key, [b]);
    } else {
      leaves.push(b);
    }
  }
  const nodes: TreeNode[] = [];
  for (const [label, items] of folders) {
    // A folder holding a single branch is noise — inline the branch instead.
    if (items.length === 1) {
      nodes.push({ kind: "leaf", branch: items[0]! });
      continue;
    }
    nodes.push({ kind: "folder", label, children: buildTree(items, depth + 1) });
  }
  for (const b of leaves) nodes.push({ kind: "leaf", branch: b });
  return nodes;
}

function BranchTree({
  branches,
  flat,
  onPick,
  onFlyout,
}: {
  branches: BranchDetail[];
  flat: boolean;
  onPick: (b: BranchDetail) => void;
  onFlyout: (b: BranchDetail, rect: DOMRect) => void;
}) {
  const nodes = useMemo(
    () =>
      flat
        ? branches.map((b): TreeNode => ({ kind: "leaf", branch: b }))
        : buildTree(branches, 0),
    [branches, flat],
  );
  return (
    <>
      {nodes.map((n) => (
        <TreeRow
          key={n.kind === "leaf" ? `b:${n.branch.name}` : `f:${n.label}`}
          node={n}
          depth={0}
          onPick={onPick}
          onFlyout={onFlyout}
        />
      ))}
    </>
  );
}

function TreeRow({
  node,
  depth,
  onPick,
  onFlyout,
}: {
  node: TreeNode;
  depth: number;
  onPick: (b: BranchDetail) => void;
  onFlyout: (b: BranchDetail, rect: DOMRect) => void;
}) {
  const [open, setOpen] = useState(true);
  if (node.kind === "leaf") {
    return (
      <BranchRow
        branch={node.branch}
        depth={depth}
        onPick={() => onPick(node.branch)}
        onFlyout={(rect) => onFlyout(node.branch, rect)}
      />
    );
  }
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1 py-0.5 pr-2 text-left text-[11px] text-neutral-400 hover:bg-neutral-900"
        style={{ paddingLeft: 12 + depth * 12 }}
      >
        {open ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
        <Folder size={10} className="text-neutral-600" />
        {node.label}
      </button>
      {open &&
        node.children.map((c) => (
          <TreeRow
            key={c.kind === "leaf" ? `b:${c.branch.name}` : `f:${c.label}`}
            node={c}
            depth={depth + 1}
            onPick={onPick}
            onFlyout={onFlyout}
          />
        ))}
    </>
  );
}

function BranchRow({
  branch,
  depth,
  onPick,
  onFlyout,
}: {
  branch: BranchDetail;
  depth: number;
  onPick: () => void;
  onFlyout: (rect: DOMRect) => void;
}) {
  const { t } = useTranslation("git");
  const blocked = branch.checked_out_in !== null;
  // Only the trailing segment — the folder rows already carry the prefix.
  const parts = branch.name.split("/");
  const leaf = parts[parts.length - 1] ?? branch.name;
  return (
    <div
      className={`group flex items-center gap-1 pr-1 text-[11px] ${
        branch.is_current
          ? "bg-neutral-900/60 text-emerald-300"
          : "text-neutral-200 hover:bg-neutral-900"
      }`}
      style={{ paddingLeft: 12 + depth * 12 }}
    >
      <button
        type="button"
        onClick={onPick}
        disabled={blocked}
        title={
          blocked
            ? t("checked_out_in", { name: branch.checked_out_in })
            : `${branch.name} · ${branch.tip_short} ${branch.tip_summary}`
        }
        className="flex min-w-0 flex-1 items-center gap-1 py-0.5 text-left disabled:opacity-50"
      >
        {branch.is_current ? (
          <Check size={10} className="shrink-0 text-emerald-400" />
        ) : (
          <GitBranch size={10} className="shrink-0 text-neutral-600" />
        )}
        <span className="truncate">{leaf}</span>
        {branch.ahead_behind &&
          (branch.ahead_behind.ahead > 0 || branch.ahead_behind.behind > 0) && (
            <span className="shrink-0 text-[9px] text-neutral-500">
              {branch.ahead_behind.ahead > 0 && `↑${branch.ahead_behind.ahead}`}
              {branch.ahead_behind.behind > 0 && `↓${branch.ahead_behind.behind}`}
            </span>
          )}
        {blocked && (
          <span className="shrink-0 truncate text-[9px] text-amber-500/80">
            {branch.checked_out_in}
          </span>
        )}
      </button>
      {branch.upstream && (
        <span className="shrink-0 truncate text-[9px] text-neutral-600">
          {branch.upstream}
        </span>
      )}
      <button
        type="button"
        onClick={(e) =>
          onFlyout((e.currentTarget as HTMLElement).getBoundingClientRect())
        }
        aria-label={t("branch_actions")}
        title={t("branch_actions")}
        className="shrink-0 rounded p-0.5 text-neutral-500 opacity-0 hover:bg-neutral-800 hover:text-neutral-200 group-hover:opacity-100"
      >
        <ChevronRight size={11} />
      </button>
    </div>
  );
}

// ────── per-branch action flyout ───────────────────────────────────────────

function BranchActions({
  projectId,
  worktreeId,
  branch,
  current,
  x,
  y,
  onCompare,
  onRename,
  onNewBranchFrom,
  onDone,
  onClose,
}: {
  projectId: string;
  worktreeId: string;
  branch: BranchDetail;
  current: BranchDetail | null;
  x: number;
  y: number;
  onCompare: (from: string, to: string, title: string) => void;
  onRename: (name: string) => void;
  onNewBranchFrom: (name: string) => void;
  onDone: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation("git");
  const checkout = useGitStore((s) => s.checkout);
  const checkoutRemote = useGitStore((s) => s.checkoutRemoteBranch);
  const merge = useGitStore((s) => s.mergeBranch);
  const rebase = useGitStore((s) => s.rebaseOnto);
  const del = useGitStore((s) => s.deleteBranch);
  const delRemote = useGitStore((s) => s.deleteRemoteBranch);
  const pull = useGitStore((s) => s.pull);
  const push = useGitStore((s) => s.push);
  // Shared placement: flips / clamps / scrolls so the popup never spills out of
  // the viewport. The same node backs the outside-click check below.
  const { ref, style } = useAnchoredMenu<HTMLDivElement>(x, y);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    // Capture phase: the popup's own outside-click handler would otherwise
    // close the whole menu on the same event.
    window.addEventListener("mousedown", onDown, true);
    return () => window.removeEventListener("mousedown", onDown, true);
  }, [onClose]);

  const currentName = current?.name ?? "HEAD";
  const isCurrent = branch.is_current;

  const runMerge = async (noFf: boolean) => {
    onDone();
    try {
      const outcome = await merge(projectId, worktreeId, branch.name, noFf);
      if (outcome.kind === "conflicts") {
        window.alert(
          t("merge_conflicts", { count: outcome.paths.length }),
        );
      }
    } catch {
      /* store already surfaced the error in the panel banner */
    }
  };

  const runRebase = async () => {
    onDone();
    try {
      const outcome = await rebase(projectId, worktreeId, branch.name);
      if (outcome.kind === "conflicts") {
        window.alert(t("rebase_conflicts", { count: outcome.paths.length }));
      }
    } catch {
      /* surfaced in the panel banner */
    }
  };

  return (
    <div
      ref={ref}
      style={style}
      className="fixed z-40 w-[250px] rounded border border-neutral-800 bg-neutral-950 py-1 text-[11px] shadow-xl"
    >
      <div className="truncate border-b border-neutral-800 px-3 pb-1 text-[10px] text-neutral-500">
        {branch.name}
      </div>

      {!isCurrent && (
        <Item
          icon={<Check size={11} />}
          label={branch.is_remote ? t("checkout_as_local") : t("checkout")}
          disabled={branch.checked_out_in !== null}
          onClick={() => {
            onDone();
            if (branch.is_remote) {
              void checkoutRemote(projectId, worktreeId, branch.name);
            } else {
              void checkout(projectId, worktreeId, branch.name);
            }
          }}
        />
      )}
      {isCurrent && (
        <>
          <Item
            icon={<ArrowDownToLine size={11} />}
            label={t("pull")}
            onClick={() => {
              onDone();
              void pull(projectId, worktreeId, false);
            }}
          />
          <Item
            icon={<ArrowUpToLine size={11} />}
            label={t("push")}
            onClick={() => {
              onDone();
              void push(projectId, worktreeId, true);
            }}
          />
        </>
      )}
      <Item
        icon={<Plus size={11} />}
        label={t("new_branch_from_short")}
        onClick={() => onNewBranchFrom(branch.name)}
      />

      {!isCurrent && (
        <>
          <Divider />
          <Item
            icon={<GitMerge size={11} />}
            label={t("merge_into", { from: branch.name, into: currentName })}
            onClick={() => void runMerge(false)}
          />
          <Item
            icon={<GitMerge size={11} />}
            label={t("merge_into_no_ff")}
            onClick={() => void runMerge(true)}
          />
          <Item
            icon={<GitBranch size={11} />}
            label={t("rebase_onto", { current: currentName, onto: branch.name })}
            onClick={() => void runRebase()}
          />
          <Divider />
          <Item
            icon={<FileDiffIcon size={11} />}
            label={t("compare_with_current", { name: branch.name })}
            onClick={() => {
              onDone();
              onCompare(
                branch.name,
                "HEAD",
                t("rev_compare_generic", { from: branch.name, to: currentName }),
              );
            }}
          />
          <Item
            icon={<FileDiffIcon size={11} />}
            label={t("compare_with_worktree")}
            onClick={() => {
              onDone();
              onCompare(
                branch.name,
                "WORKTREE",
                t("rev_compare_generic", {
                  from: branch.name,
                  to: t("working_tree"),
                }),
              );
            }}
          />
        </>
      )}

      <Divider />
      {!branch.is_remote && (
        <Item
          icon={<Pencil size={11} />}
          label={t("rename")}
          onClick={() => onRename(branch.name)}
        />
      )}
      {!branch.is_remote && !isCurrent && (
        <Item
          icon={<Trash2 size={11} />}
          danger
          label={t("delete_branch")}
          onClick={() => {
            if (!window.confirm(t("delete_branch_confirm", { name: branch.name })))
              return;
            onDone();
            void del(projectId, worktreeId, branch.name);
          }}
        />
      )}
      {branch.is_remote && (
        <Item
          icon={<Trash2 size={11} />}
          danger
          label={t("delete_on_remote")}
          onClick={() => {
            if (
              !window.confirm(t("delete_remote_confirm", { name: branch.name }))
            )
              return;
            onDone();
            void delRemote(projectId, worktreeId, branch.name, true);
          }}
        />
      )}
    </div>
  );
}

function Item({
  icon,
  label,
  onClick,
  disabled,
  danger,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={label}
      className={`flex w-full items-center gap-2 px-3 py-1 text-left disabled:opacity-40 ${
        danger
          ? "text-red-300 enabled:hover:bg-red-900/30"
          : "text-neutral-200 enabled:hover:bg-neutral-900"
      }`}
    >
      <span className={danger ? "text-red-400" : "text-neutral-500"}>{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
    </button>
  );
}
