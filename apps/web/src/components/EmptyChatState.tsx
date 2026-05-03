import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Folder, GitBranch, Plus, Star } from "lucide-react";
import {
  type WorktreeRow,
  worktreeCreate,
  WorktreeCommandError,
} from "~/ipc/worktree.ts";
import { runAutoActionsOnWorktreeCreate } from "~/lib/runAutoActions.ts";

interface Props {
  projectId: string;
  projectName: string;
  worktrees: WorktreeRow[];
  loading: boolean;
  selectedWorktreeId: string | null;
  onSelectWorktree: (id: string | null) => void;
  onWorktreesChanged: () => void;
}

/**
 * Shown when a project is active but no session is selected. The user picks
 * which worktree the upcoming session will run in (or creates a new one)
 * before typing the first message in the composer below.
 */
export function EmptyChatState({
  projectId,
  projectName: _projectName,
  worktrees,
  loading,
  selectedWorktreeId,
  onSelectWorktree,
  onWorktreesChanged,
}: Props) {
  const { t } = useTranslation("common");
  const [branch, setBranch] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onCreate = async () => {
    const trimmed = branch.trim();
    if (!trimmed || creating) return;
    setCreating(true);
    setError(null);
    try {
      const created = await worktreeCreate({
        project_id: projectId,
        branch: trimmed,
      });
      setBranch("");
      onWorktreesChanged();
      onSelectWorktree(created.id);
      void runAutoActionsOnWorktreeCreate({
        projectId,
        worktreeId: created.id,
        sessionId: null,
      });
    } catch (e) {
      setError(formatErr(e));
    } finally {
      setCreating(false);
    }
  };

  // Pre-select the primary worktree by default so a one-click send works.
  if (selectedWorktreeId === null && worktrees.length > 0) {
    const primary = worktrees.find((w) => w.is_primary) ?? worktrees[0];
    if (primary) {
      // Defer state set so the parent can render this initial selection.
      queueMicrotask(() => onSelectWorktree(primary.id));
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-6 py-10">
      <div className="mx-auto flex w-full max-w-2xl flex-col gap-8">
        <div className="text-center">
          <h1 className="text-2xl font-semibold tracking-tight text-neutral-100">
            {t("empty_state.heading")}
          </h1>
          <p className="mt-2 text-sm text-neutral-400">
            {t("empty_state.subheading")}
          </p>
        </div>

        <section>
          {loading && worktrees.length === 0 ? (
            <p className="text-center text-sm text-neutral-500">
              {t("empty_state.loading")}
            </p>
          ) : (
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              {worktrees.map((w) => {
                const selected = selectedWorktreeId === w.id;
                return (
                  <button
                    key={w.id}
                    type="button"
                    onClick={() => onSelectWorktree(w.id)}
                    className={`flex flex-col items-start gap-2 rounded-xl border p-4 text-left transition ${
                      selected
                        ? "border-emerald-700/70 bg-emerald-950/20 ring-1 ring-emerald-800/60"
                        : "border-neutral-800 bg-neutral-900/40 hover:border-neutral-700 hover:bg-neutral-900/70"
                    }`}
                  >
                    <div className="flex w-full items-center gap-2">
                      {w.is_primary ? (
                        <Star
                          className="size-4 shrink-0 text-amber-300"
                          strokeWidth={1.75}
                        />
                      ) : (
                        <GitBranch
                          className="size-4 shrink-0 text-neutral-400"
                          strokeWidth={1.75}
                        />
                      )}
                      <span className="min-w-0 flex-1 truncate text-sm font-medium text-neutral-100">
                        {w.branch}
                      </span>
                      {w.is_primary && (
                        <span className="rounded bg-amber-950/40 px-1.5 py-0.5 text-[9px] uppercase tracking-wider text-amber-300">
                          {t("empty_state.primary_badge")}
                        </span>
                      )}
                    </div>
                    <span className="block w-full truncate text-[11px] text-neutral-500">
                      <Folder
                        className="-mt-px mr-1 inline size-3 align-middle"
                        strokeWidth={1.75}
                      />
                      {w.path}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </section>

        <section className="rounded-xl border border-dashed border-neutral-800 bg-neutral-900/30 p-4">
          <div className="mb-3 flex items-center gap-2 text-[11px] uppercase tracking-wider text-neutral-500">
            <Plus className="size-3" strokeWidth={1.75} />
            {t("empty_state.new_worktree_heading")}
          </div>
          <label className="block text-[11px] text-neutral-500">
            {t("empty_state.new_worktree_branch_label")}
          </label>
          <div className="mt-1 flex gap-2">
            <input
              type="text"
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void onCreate();
                }
              }}
              placeholder={t("empty_state.new_worktree_branch_placeholder")}
              disabled={creating}
              className="flex-1 rounded-md border border-neutral-800 bg-neutral-950 px-2.5 py-1.5 text-sm text-neutral-100 placeholder:text-neutral-600 outline-none focus:border-neutral-700 disabled:opacity-50"
            />
            <button
              type="button"
              onClick={() => void onCreate()}
              disabled={!branch.trim() || creating}
              className="rounded-md bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-900 transition hover:bg-white disabled:cursor-not-allowed disabled:bg-neutral-800 disabled:text-neutral-500"
            >
              {creating
                ? t("empty_state.creating")
                : t("empty_state.new_worktree_submit")}
            </button>
          </div>
          {error && (
            <p className="mt-2 rounded border border-red-900/40 bg-red-950/20 px-2 py-1 text-[11px] text-red-200">
              {t("empty_state.error_prefix", { message: error })}
            </p>
          )}
        </section>
      </div>
    </div>
  );
}

function formatErr(e: unknown): string {
  if (e instanceof WorktreeCommandError) {
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}
