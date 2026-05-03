import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowDownToLine,
  ArrowUpToLine,
  Check,
  ChevronDown,
  ChevronRight,
  Download,
  GitBranch,
  GitCommit,
  History,
  Inbox,
  Minus,
  Plus,
  RefreshCw,
  Sparkles,
  Tag,
  Undo2,
} from "lucide-react";
import {
  PRIMARY_WORKTREE_ID,
  worktreeList,
  type WorktreeRow,
} from "~/ipc/worktree.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import {
  partitionByBucket,
  useGitStore,
} from "~/stores/gitStore.ts";
import type { DiffMode, StatusEntry } from "~/ipc/git.ts";
import { MonacoDiffViewer } from "~/components/MonacoDiffViewer.tsx";
import { MergeEditor } from "~/components/MergeEditor.tsx";
import {
  buildSingleHunkPatch,
  parseUnifiedDiff,
  type Hunk,
} from "~/lib/diff-hunks.ts";

interface Props {
  projectId: string | null;
}

// Stable empty references — selectors that synthesize a new `[]` / `{}`
// per call trigger a zustand re-render storm under React 19 strict checks.
const EMPTY_BRANCHES: { name: string; is_current: boolean; is_remote: boolean }[] = [];
const EMPTY_LOG: never[] = [];
const EMPTY_DIFFS: Record<string, never> = {};
const EMPTY_DIFF_LOADING: Record<string, boolean> = {};
const EMPTY_STASHES: never[] = [];
const EMPTY_TAGS: never[] = [];

export function GitPanel({ projectId }: Props) {
  const { t } = useTranslation("git");
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const sessionSnapshot = useSessionStore((s) =>
    activeSessionId ? s.snapshots[activeSessionId] : null,
  );
  const [worktrees, setWorktrees] = useState<WorktreeRow[]>([]);
  const [overrides, setOverrides] = useState<Record<string, string>>({});

  const sessionWorktreeId = sessionSnapshot?.worktree_id ?? PRIMARY_WORKTREE_ID;
  const worktreeId =
    (projectId && overrides[projectId]) ||
    sessionWorktreeId ||
    PRIMARY_WORKTREE_ID;

  useEffect(() => {
    if (!projectId) return;
    let cancelled = false;
    void worktreeList({ project_id: projectId }).then((rows) => {
      if (!cancelled) setWorktrees(rows);
    });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  if (!projectId) {
    return (
      <div className="flex h-full flex-1 items-center justify-center text-[12px] text-neutral-500">
        {t("pick_project")}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-1">
      <div className="flex w-80 shrink-0 flex-col border-r border-neutral-800 bg-neutral-950">
        <div className="flex items-center gap-1 border-b border-neutral-800 px-2 py-1.5">
          <select
            value={worktreeId}
            onChange={(e) =>
              setOverrides((prev) => ({ ...prev, [projectId]: e.target.value }))
            }
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
        </div>
        <BranchToolbar projectId={projectId} worktreeId={worktreeId} />
        <GitChangesList projectId={projectId} worktreeId={worktreeId} />
        <LogSection projectId={projectId} worktreeId={worktreeId} />
        <CommitBar projectId={projectId} worktreeId={worktreeId} />
      </div>
      <GitDiffPane projectId={projectId} worktreeId={worktreeId} />
    </div>
  );
}

function BranchToolbar({
  projectId,
  worktreeId,
}: {
  projectId: string;
  worktreeId: string;
}) {
  const { t } = useTranslation("git");
  const branches = useGitStore(
    (s) => s.branches[worktreeId] ?? EMPTY_BRANCHES,
  );
  const refreshBranches = useGitStore((s) => s.refreshBranches);
  const checkout = useGitStore((s) => s.checkout);
  const createBranch = useGitStore((s) => s.createBranch);
  const fetch = useGitStore((s) => s.fetch);
  const pull = useGitStore((s) => s.pull);
  const push = useGitStore((s) => s.push);
  const remote = useGitStore((s) => s.remote[worktreeId]);
  const status = useGitStore((s) => s.status[worktreeId]);
  const [picker, setPicker] = useState(false);
  const current = branches.find((b) => b.is_current);

  useEffect(() => {
    void refreshBranches(projectId, worktreeId);
  }, [projectId, worktreeId, refreshBranches]);

  return (
    <div className="border-b border-neutral-800 bg-neutral-900/40">
      <div className="flex items-center justify-between gap-1 px-2 py-1 text-[11px]">
        <div className="relative min-w-0 flex-1">
          <button
            type="button"
            onClick={() => setPicker((v) => !v)}
            className="flex w-full items-center gap-1 truncate rounded bg-neutral-900 px-2 py-0.5 text-neutral-200 hover:bg-neutral-800"
          >
            <GitBranch size={11} className="shrink-0 text-neutral-500" />
            <span className="truncate">
              {current?.name ?? status?.branch ?? t("no_branch")}
            </span>
            <ChevronDown size={11} className="ml-auto shrink-0 text-neutral-500" />
          </button>
          {picker && (
            <BranchPicker
              branches={branches}
              onPick={async (name) => {
                setPicker(false);
                await checkout(projectId, worktreeId, name);
              }}
              onCreate={async (name) => {
                setPicker(false);
                await createBranch(projectId, worktreeId, name, true);
              }}
              onClose={() => setPicker(false)}
            />
          )}
        </div>
        <div className="flex items-center gap-0.5">
          <button
            type="button"
            onClick={() => void fetch(projectId, worktreeId)}
            disabled={remote?.running}
            className="rounded p-1 text-neutral-400 enabled:hover:bg-neutral-800 enabled:hover:text-neutral-100 disabled:opacity-40"
            title={t("fetch")}
            aria-label={t("fetch")}
          >
            <Download size={11} />
          </button>
          <button
            type="button"
            onClick={() => void pull(projectId, worktreeId, false)}
            disabled={remote?.running}
            className="rounded p-1 text-neutral-400 enabled:hover:bg-neutral-800 enabled:hover:text-neutral-100 disabled:opacity-40"
            title={t("pull")}
            aria-label={t("pull")}
          >
            <ArrowDownToLine size={11} />
          </button>
          <button
            type="button"
            onClick={() => void push(projectId, worktreeId, false)}
            disabled={remote?.running}
            className="rounded p-1 text-neutral-400 enabled:hover:bg-neutral-800 enabled:hover:text-neutral-100 disabled:opacity-40"
            title={t("push")}
            aria-label={t("push")}
          >
            <ArrowUpToLine size={11} />
          </button>
          <StashButton projectId={projectId} worktreeId={worktreeId} />
        </div>
      </div>
      {remote?.error && (
        <div
          className="border-t border-red-900/40 bg-red-900/20 px-2 py-1 text-[10px] text-red-300"
          role="alert"
        >
          {remote.error}
        </div>
      )}
      {remote?.lastOutput && !remote.error && (
        <div className="border-t border-neutral-800/60 bg-neutral-950 px-2 py-1 text-[10px] text-neutral-500">
          <pre className="max-h-20 overflow-auto whitespace-pre-wrap font-mono">
            {remote.lastOutput.trim()}
          </pre>
        </div>
      )}
    </div>
  );
}

function StashButton({
  projectId,
  worktreeId,
}: {
  projectId: string;
  worktreeId: string;
}) {
  const { t, i18n } = useTranslation("git");
  const stashes = useGitStore((s) => s.stashes[worktreeId] ?? EMPTY_STASHES);
  const refreshStashes = useGitStore((s) => s.refreshStashes);
  const saveStash = useGitStore((s) => s.saveStash);
  const applyStash = useGitStore((s) => s.applyStash);
  const dropStash = useGitStore((s) => s.dropStash);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (open) void refreshStashes(projectId, worktreeId);
  }, [open, projectId, worktreeId, refreshStashes]);

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex items-center gap-1 rounded p-1 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
        title={t("stash")}
        aria-label={t("stash")}
      >
        <Inbox size={11} />
        {stashes.length > 0 && (
          <span className="text-[9px] text-neutral-500">{stashes.length}</span>
        )}
      </button>
      {open && (
        <div
          onMouseLeave={() => setOpen(false)}
          className="absolute right-0 top-full z-20 mt-1 w-72 rounded border border-neutral-800 bg-neutral-950 p-1 shadow-lg"
        >
          <div className="border-b border-neutral-800 p-1">
            <input
              type="text"
              placeholder={t("stash_placeholder")}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  const v = (e.target as HTMLInputElement).value.trim();
                  if (v) {
                    void saveStash(projectId, worktreeId, v, true).then(() =>
                      ((e.target as HTMLInputElement).value = ""),
                    );
                  }
                }
              }}
              className="w-full rounded bg-neutral-900 px-2 py-0.5 text-[11px] text-neutral-200 outline-none focus:ring-1 focus:ring-neutral-700"
            />
          </div>
          {stashes.length === 0 && (
            <div className="px-2 py-2 text-[11px] text-neutral-500">
              {t("no_stashes")}
            </div>
          )}
          {stashes.map((s) => (
            <div
              key={s.index}
              className="group flex items-center gap-1 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-900"
            >
              <code className="text-neutral-500">{s.short_id}</code>
              <span className="min-w-0 flex-1 truncate">{s.message}</span>
              <span className="text-[9px] text-neutral-500">
                {new Date(s.time * 1000).toLocaleDateString(i18n.language)}
              </span>
              <button
                type="button"
                onClick={() => void applyStash(projectId, worktreeId, s.index, false)}
                className="opacity-0 group-hover:opacity-100 rounded px-1 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
                title={t("stash_apply")}
              >
                <Check size={11} />
              </button>
              <button
                type="button"
                onClick={() => {
                  if (window.confirm(t("stash_drop_confirm", { id: s.short_id }))) {
                    void dropStash(projectId, worktreeId, s.index);
                  }
                }}
                className="opacity-0 group-hover:opacity-100 rounded px-1 text-red-300 hover:bg-red-900/30"
                title={t("stash_drop")}
              >
                <Minus size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function BranchPicker({
  branches,
  onPick,
  onCreate,
  onClose,
}: {
  branches: { name: string; is_current: boolean; is_remote: boolean }[];
  onPick: (name: string) => void;
  onCreate: (name: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation("git");
  const [filter, setFilter] = useState("");
  const [creating, setCreating] = useState(false);
  const filtered = useMemo(
    () =>
      branches.filter((b) =>
        b.name.toLowerCase().includes(filter.toLowerCase()),
      ),
    [branches, filter],
  );
  return (
    <div
      onMouseLeave={onClose}
      className="absolute left-0 top-full z-20 mt-1 max-h-72 w-72 overflow-auto rounded border border-neutral-800 bg-neutral-950 p-1 shadow-lg"
    >
      <input
        type="text"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        placeholder={t("branch_search")}
        className="mb-1 w-full rounded bg-neutral-900 px-2 py-0.5 text-[11px] text-neutral-200 outline-none focus:ring-1 focus:ring-neutral-700"
      />
      {filtered.map((b) => (
        <button
          key={`${b.is_remote ? "r" : "l"}-${b.name}`}
          type="button"
          onClick={() => onPick(b.name)}
          className="flex w-full items-center gap-1 rounded px-2 py-0.5 text-left text-[11px] text-neutral-200 hover:bg-neutral-900"
        >
          {b.is_current ? (
            <Check size={10} className="text-emerald-400" />
          ) : (
            <span className="w-[10px]" />
          )}
          <span className="truncate">{b.name}</span>
          {b.is_remote && (
            <span className="ml-auto text-[9px] text-neutral-500">remote</span>
          )}
        </button>
      ))}
      <div className="mt-1 border-t border-neutral-800 pt-1">
        {creating ? (
          <input
            type="text"
            autoFocus
            placeholder={t("new_branch_placeholder")}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const v = (e.target as HTMLInputElement).value.trim();
                if (v) onCreate(v);
              } else if (e.key === "Escape") {
                setCreating(false);
              }
            }}
            className="w-full rounded bg-neutral-900 px-2 py-0.5 text-[11px] text-neutral-200 outline-none focus:ring-1 focus:ring-neutral-700"
          />
        ) : (
          <button
            type="button"
            onClick={() => setCreating(true)}
            className="flex w-full items-center gap-1 rounded px-2 py-0.5 text-left text-[11px] text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
          >
            <Plus size={10} />
            {t("new_branch")}
          </button>
        )}
      </div>
    </div>
  );
}

function LogSection({
  projectId,
  worktreeId,
}: {
  projectId: string;
  worktreeId: string;
}) {
  const { t, i18n } = useTranslation("git");
  const log = useGitStore((s) => s.log[worktreeId] ?? EMPTY_LOG);
  const refreshLog = useGitStore((s) => s.refreshLog);
  const tags = useGitStore((s) => s.tags[worktreeId] ?? EMPTY_TAGS);
  const refreshTags = useGitStore((s) => s.refreshTags);
  const cherryPick = useGitStore((s) => s.cherryPick);
  const revertCommit = useGitStore((s) => s.revertCommit);
  const createTag = useGitStore((s) => s.createTag);
  const [open, setOpen] = useState(false);
  const [menu, setMenu] = useState<
    { x: number; y: number; oid: string; short: string } | null
  >(null);

  useEffect(() => {
    if (open) {
      if (log.length === 0) void refreshLog(projectId, worktreeId, 50);
      void refreshTags(projectId, worktreeId);
    }
  }, [open, log.length, projectId, worktreeId, refreshLog, refreshTags]);

  useEffect(() => {
    if (!menu) return;
    const onDown = () => setMenu(null);
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [menu]);

  // Index tags by commit OID so each row can show its tag chips.
  const tagsByOid = useMemo(() => {
    const out: Record<string, string[]> = {};
    for (const tg of tags) {
      (out[tg.oid] ??= []).push(tg.name);
    }
    return out;
  }, [tags]);

  return (
    <div className="border-t border-neutral-800 bg-neutral-950">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1 px-2 py-1 text-[10px] uppercase tracking-wide text-neutral-500 hover:text-neutral-300"
      >
        {open ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
        <History size={11} />
        {t("history")}
      </button>
      {open && (
        <div className="max-h-48 overflow-auto border-t border-neutral-800/60 px-2 py-1 text-[11px]">
          {log.length === 0 && (
            <div className="text-neutral-500">{t("no_commits")}</div>
          )}
          {log.map((c) => (
            <div
              key={c.oid}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({
                  x: e.clientX,
                  y: e.clientY,
                  oid: c.oid,
                  short: c.short_oid,
                });
              }}
              className="border-b border-neutral-900/50 py-1 hover:bg-neutral-900/40"
            >
              <div className="flex items-baseline gap-2">
                <code className="text-neutral-500">{c.short_oid}</code>
                {(tagsByOid[c.oid] ?? []).map((name) => (
                  <span
                    key={name}
                    className="rounded bg-emerald-900/40 px-1 text-[9px] text-emerald-300"
                  >
                    <Tag size={8} className="mr-0.5 inline" />
                    {name}
                  </span>
                ))}
                <span className="truncate text-neutral-200">{c.summary}</span>
              </div>
              <div className="text-[10px] text-neutral-500">
                {c.author_name} ·{" "}
                {new Date(c.author_time * 1000).toLocaleString(i18n.language)}
              </div>
            </div>
          ))}
        </div>
      )}
      {menu && (
        <div
          style={{ left: menu.x, top: menu.y }}
          className="fixed z-50 min-w-[180px] rounded border border-neutral-800 bg-neutral-950 py-1 text-[11px] shadow-lg"
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            onClick={async () => {
              setMenu(null);
              const oid = await cherryPick(projectId, worktreeId, menu.oid);
              if (oid === null) {
                window.alert(t("cherry_conflict"));
              }
            }}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            <GitCommit size={11} />
            {t("cherry_pick", { id: menu.short })}
          </button>
          <button
            type="button"
            onClick={async () => {
              setMenu(null);
              const oid = await revertCommit(projectId, worktreeId, menu.oid);
              if (oid === null) {
                window.alert(t("revert_conflict"));
              }
            }}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            <Undo2 size={11} />
            {t("revert", { id: menu.short })}
          </button>
          <div className="my-1 border-t border-neutral-800" />
          <button
            type="button"
            onClick={() => {
              const oid = menu.oid;
              setMenu(null);
              const name = window.prompt(t("tag_name_prompt"));
              if (!name) return;
              const message = window.prompt(t("tag_message_prompt")) ?? undefined;
              void createTag(projectId, worktreeId, name, oid, message);
            }}
            className="flex w-full items-center gap-2 px-3 py-1 text-left text-neutral-200 hover:bg-neutral-900"
          >
            <Tag size={11} />
            {t("tag_here")}
          </button>
        </div>
      )}
    </div>
  );
}

function GitChangesList({
  projectId,
  worktreeId,
}: {
  projectId: string;
  worktreeId: string;
}) {
  const { t } = useTranslation("git");
  const status = useGitStore((s) => s.status[worktreeId] ?? null);
  const loading = useGitStore((s) => s.loading[worktreeId] ?? false);
  const error = useGitStore((s) => s.error[worktreeId] ?? null);
  const refreshStatus = useGitStore((s) => s.refreshStatus);
  const stagePaths = useGitStore((s) => s.stagePaths);
  const unstagePaths = useGitStore((s) => s.unstagePaths);
  const selectDiff = useGitStore((s) => s.selectDiff);
  const selected = useGitStore((s) => s.selected[worktreeId] ?? null);

  useEffect(() => {
    void refreshStatus(projectId, worktreeId);
  }, [projectId, worktreeId, refreshStatus]);

  const sections = useMemo(
    () => (status ? partitionByBucket(status.entries) : null),
    [status],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between border-b border-neutral-800/60 bg-neutral-900/30 px-2 py-1 text-[11px] text-neutral-300">
        <span className="flex items-center gap-1">
          <GitBranch size={12} className="text-neutral-500" />
          {status?.branch ?? t("no_branch")}
          {status?.ahead_behind && (
            <span className="ml-1 text-neutral-500">
              ↑{status.ahead_behind.ahead} ↓{status.ahead_behind.behind}
            </span>
          )}
        </span>
        <button
          type="button"
          onClick={() => void refreshStatus(projectId, worktreeId)}
          className="rounded p-1 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
          title={t("refresh")}
          aria-label={t("refresh")}
          disabled={loading}
        >
          <RefreshCw size={11} className={loading ? "animate-spin" : ""} />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto py-1 text-[12px]">
        {error && (
          <div className="px-3 py-2 text-red-400" role="alert">
            {error}
          </div>
        )}
        {!sections && !loading && !error && (
          <div className="px-3 py-2 text-neutral-500">{t("no_repo")}</div>
        )}
        {sections && (
          <>
            <Section
              label={t("staged")}
              entries={sections.staged}
              selected={selected}
              onClick={(e) =>
                void selectDiff(projectId, worktreeId, e.path, "staged_vs_head")
              }
              onAction={(paths) =>
                void unstagePaths(projectId, worktreeId, paths)
              }
              actionIcon={<Minus size={11} />}
              actionLabel={t("unstage")}
              defaultMode="staged_vs_head"
            />
            <Section
              label={t("unstaged")}
              entries={sections.unstaged}
              selected={selected}
              onClick={(e) =>
                void selectDiff(
                  projectId,
                  worktreeId,
                  e.path,
                  "working_vs_staged",
                )
              }
              onAction={(paths) =>
                void stagePaths(projectId, worktreeId, paths)
              }
              actionIcon={<Plus size={11} />}
              actionLabel={t("stage")}
              defaultMode="working_vs_staged"
            />
            <Section
              label={t("untracked")}
              entries={sections.untracked}
              selected={selected}
              onClick={(e) =>
                void selectDiff(projectId, worktreeId, e.path, "working_vs_head")
              }
              onAction={(paths) =>
                void stagePaths(projectId, worktreeId, paths)
              }
              actionIcon={<Plus size={11} />}
              actionLabel={t("stage")}
              defaultMode="working_vs_head"
            />
            {sections.conflicted.length > 0 && (
              <Section
                label={t("conflicted")}
                entries={sections.conflicted}
                selected={selected}
                onClick={(e) =>
                  void selectDiff(
                    projectId,
                    worktreeId,
                    e.path,
                    "working_vs_head",
                  )
                }
                onAction={() => {
                  /* fase 3: merge editor */
                }}
                actionIcon={null}
                actionLabel={t("conflicted")}
                defaultMode="working_vs_head"
              />
            )}
          </>
        )}
      </div>
    </div>
  );
}

interface SectionProps {
  label: string;
  entries: StatusEntry[];
  selected: { path: string; mode: DiffMode } | null;
  onClick: (e: StatusEntry) => void;
  onAction: (paths: string[]) => void;
  actionIcon: React.ReactNode | null;
  actionLabel: string;
  defaultMode: DiffMode;
}

function Section({
  label,
  entries,
  selected,
  onClick,
  onAction,
  actionIcon,
  actionLabel,
  defaultMode,
}: SectionProps) {
  const [open, setOpen] = useState(true);
  if (entries.length === 0) return null;
  return (
    <div className="mb-1">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center justify-between gap-1 px-2 py-0.5 text-[10px] uppercase tracking-wide text-neutral-500 hover:text-neutral-300"
      >
        <span className="flex items-center gap-1">
          {open ? (
            <ChevronDown size={10} />
          ) : (
            <ChevronRight size={10} />
          )}
          {label}
          <span className="text-neutral-600">({entries.length})</span>
        </span>
        {actionIcon && entries.length > 0 && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onAction(entries.map((x) => x.path));
            }}
            className="rounded px-1 py-0.5 text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
            aria-label={actionLabel}
            title={actionLabel}
          >
            {actionIcon}
          </button>
        )}
      </button>
      {open &&
        entries.map((entry) => {
          const isSelected =
            selected?.path === entry.path && selected?.mode === defaultMode;
          return (
            <div
              key={`${entry.bucket}-${entry.path}`}
              className={`group flex items-center gap-1 px-2 py-0.5 ${
                isSelected
                  ? "bg-neutral-900 text-neutral-100"
                  : "text-neutral-300 hover:bg-neutral-900/60"
              }`}
            >
              <span
                className={`shrink-0 rounded px-1 text-[9px] ${statusBadgeClass(entry.status)}`}
              >
                {statusBadgeChar(entry.status)}
              </span>
              <button
                type="button"
                onClick={() => onClick(entry)}
                className="min-w-0 flex-1 truncate text-left"
                title={entry.old_path ? `${entry.old_path} → ${entry.path}` : entry.path}
              >
                {entry.path}
              </button>
              {actionIcon && (
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation();
                    onAction([entry.path]);
                  }}
                  className="opacity-0 group-hover:opacity-100 rounded px-1 py-0.5 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
                  aria-label={actionLabel}
                  title={actionLabel}
                >
                  {actionIcon}
                </button>
              )}
            </div>
          );
        })}
    </div>
  );
}

function statusBadgeChar(s: StatusEntry["status"]): string {
  switch (s) {
    case "added":
      return "A";
    case "modified":
      return "M";
    case "deleted":
      return "D";
    case "renamed":
      return "R";
    case "copied":
      return "C";
    case "typechange":
      return "T";
    default:
      return "?";
  }
}

function statusBadgeClass(s: StatusEntry["status"]): string {
  switch (s) {
    case "added":
      return "bg-emerald-900/40 text-emerald-300";
    case "modified":
      return "bg-amber-900/40 text-amber-300";
    case "deleted":
      return "bg-red-900/40 text-red-300";
    case "renamed":
    case "copied":
      return "bg-blue-900/40 text-blue-300";
    default:
      return "bg-neutral-800 text-neutral-300";
  }
}

function CommitBar({
  projectId,
  worktreeId,
}: {
  projectId: string;
  worktreeId: string;
}) {
  const { t } = useTranslation("git");
  const message = useGitStore((s) => s.commitMessage[worktreeId] ?? "");
  const setMessage = useGitStore((s) => s.setCommitMessage);
  const commit = useGitStore((s) => s.commit);
  const committing = useGitStore((s) => s.committing[worktreeId] ?? false);
  const error = useGitStore((s) => s.commitError[worktreeId] ?? null);
  const status = useGitStore((s) => s.status[worktreeId] ?? null);
  const generating = useGitStore(
    (s) => s.generatingCommitMsg[worktreeId] ?? false,
  );
  const generateMsg = useGitStore((s) => s.generateCommitMessage);
  const stagedCount = useMemo(
    () => status?.entries.filter((e) => e.bucket === "staged").length ?? 0,
    [status],
  );
  const canCommit = stagedCount > 0 && message.trim().length > 0 && !committing;

  return (
    <div className="border-t border-neutral-800 bg-neutral-950 p-2">
      <div className="relative">
        <textarea
          value={message}
          onChange={(e) => setMessage(worktreeId, e.target.value)}
          placeholder={t("commit_placeholder", { count: stagedCount })}
          rows={3}
          className="w-full resize-none rounded border border-neutral-800 bg-neutral-900 px-2 py-1 pr-8 text-[12px] text-neutral-100 outline-none focus:ring-1 focus:ring-neutral-700"
        />
        <button
          type="button"
          onClick={() => void generateMsg(projectId, worktreeId)}
          disabled={stagedCount === 0 || generating}
          className="absolute right-1 top-1 rounded p-1 text-neutral-400 enabled:hover:bg-neutral-800 enabled:hover:text-amber-300 disabled:opacity-30"
          title={t("generate_msg")}
          aria-label={t("generate_msg")}
        >
          <Sparkles size={12} className={generating ? "animate-pulse" : ""} />
        </button>
      </div>
      {error && (
        <div className="mt-1 text-[11px] text-red-400" role="alert">
          {error}
        </div>
      )}
      <div className="mt-1 flex items-center justify-between gap-2 text-[11px]">
        <span className="text-neutral-500">
          {t("staged_count", { count: stagedCount })}
        </span>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => void commit(projectId, worktreeId, true)}
            disabled={committing || !message.trim()}
            className="rounded px-2 py-0.5 text-neutral-400 enabled:hover:bg-neutral-800 enabled:hover:text-neutral-200 disabled:opacity-40"
          >
            {t("amend")}
          </button>
          <button
            type="button"
            onClick={() => void commit(projectId, worktreeId, false)}
            disabled={!canCommit}
            className="rounded bg-emerald-700/80 px-2 py-0.5 text-neutral-100 enabled:hover:bg-emerald-700 disabled:opacity-40"
          >
            {committing ? t("committing") : t("commit")}
          </button>
        </div>
      </div>
    </div>
  );
}

function GitDiffPane({
  projectId,
  worktreeId,
}: {
  projectId: string;
  worktreeId: string;
}) {
  const { t } = useTranslation("git");
  const selected = useGitStore((s) => s.selected[worktreeId] ?? null);
  const status = useGitStore((s) => s.status[worktreeId] ?? null);
  const diffs = useGitStore((s) => s.diffs[worktreeId] ?? EMPTY_DIFFS);
  const diffLoading = useGitStore(
    (s) => s.diffLoading[worktreeId] ?? EMPTY_DIFF_LOADING,
  );
  const selectDiff = useGitStore((s) => s.selectDiff);
  const key = selected ? `${selected.path}::${selected.mode}` : null;
  const diff = key ? diffs[key] : null;
  const loading = key ? diffLoading[key] : false;

  const selectedEntry = useMemo(
    () =>
      selected
        ? status?.entries.find(
            (e) => e.path === selected.path && e.bucket === "conflicted",
          )
        : null,
    [selected, status],
  );

  if (!selected) {
    return (
      <div className="flex h-full flex-1 items-center justify-center text-[12px] text-neutral-500">
        {t("pick_file")}
      </div>
    );
  }

  if (selectedEntry) {
    return (
      <MergeEditor
        projectId={projectId}
        worktreeId={worktreeId}
        path={selected.path}
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-neutral-950">
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-neutral-800 px-2 text-[11px] text-neutral-400">
        <span className="truncate">{selected.path}</span>
        <div className="flex items-center gap-1">
          {(["working_vs_head", "staged_vs_head", "working_vs_staged"] as const).map(
            (mode) => (
              <button
                key={mode}
                type="button"
                onClick={() =>
                  void selectDiff(projectId, worktreeId, selected.path, mode)
                }
                className={`rounded px-1.5 py-0.5 ${
                  selected.mode === mode
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-500 hover:bg-neutral-900 hover:text-neutral-300"
                }`}
              >
                {t(`mode.${mode}`)}
              </button>
            ),
          )}
        </div>
      </div>
      <div className="flex min-h-0 flex-1 flex-col">
        {loading ? (
          <div className="p-2 text-[12px] text-neutral-500">{t("loading")}</div>
        ) : diff ? (
          <>
            {selected.mode !== "working_vs_head" && (
              <HunkBar
                projectId={projectId}
                worktreeId={worktreeId}
                diff={diff}
                reverse={selected.mode === "staged_vs_head"}
              />
            )}
            <MonacoDiffViewer
              oldContent={diff.old_content}
              newContent={diff.new_content}
              path={diff.path}
            />
          </>
        ) : null}
      </div>
    </div>
  );
}

function HunkBar({
  projectId,
  worktreeId,
  diff,
  reverse,
}: {
  projectId: string;
  worktreeId: string;
  diff: { unified: string; path: string };
  reverse: boolean;
}) {
  const { t } = useTranslation("git");
  const applyHunk = useGitStore((s) => s.applyHunk);
  const parsed = useMemo(
    () => parseUnifiedDiff(diff.unified, diff.path),
    [diff.unified, diff.path],
  );
  if (parsed.hunks.length === 0) return null;
  const onClick = async (h: Hunk) => {
    const patch = buildSingleHunkPatch(parsed, h);
    try {
      await applyHunk(projectId, worktreeId, patch, reverse);
    } catch (e) {
      window.alert(`${t("hunk_apply_failed")}: ${e instanceof Error ? e.message : e}`);
    }
  };
  return (
    <div className="flex shrink-0 flex-wrap gap-1 border-b border-neutral-800/60 bg-neutral-900/30 px-2 py-1">
      <span className="text-[10px] uppercase tracking-wide text-neutral-500">
        {t("hunks", { count: parsed.hunks.length })}
      </span>
      {parsed.hunks.map((h, i) => (
        <button
          key={`${h.header}-${i}`}
          type="button"
          onClick={() => void onClick(h)}
          className="flex items-center gap-1 rounded border border-neutral-800 bg-neutral-950 px-1.5 py-0.5 text-[10px] text-neutral-300 hover:border-neutral-700 hover:bg-neutral-900"
          title={h.header}
        >
          <span className="text-neutral-500">@{h.newStart}</span>
          <span className="text-emerald-400">+{h.added}</span>
          <span className="text-red-400">-{h.removed}</span>
          <span className="ml-1 text-neutral-400">
            {reverse ? t("unstage_hunk") : t("stage_hunk")}
          </span>
        </button>
      ))}
    </div>
  );
}
