import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { GitBranch, Plus, Star, X } from "lucide-react";
import {
  type WorktreeRow,
  WorktreeCommandError,
  worktreeCreate,
  worktreeList,
  worktreeRemove,
} from "~/ipc/worktree.ts";
import { runAutoActionsOnWorktreeCreate } from "~/lib/runAutoActions.ts";
import { useProjectStore } from "~/stores/projectStore.ts";

interface Props {
  projectId: string;
  onClose?: () => void;
}

/**
 * Project-scoped settings modal. Today: just the worktree manager moved out
 * of the sidebar. Future home for project rename, default model, runtime
 * mode, etc.
 */
export function ProjectSettingsModal({ projectId, onClose: _onClose }: Props) {
  const { t } = useTranslation("common");
  const project = useProjectStore((s) =>
    s.projects.find((p) => p.id === projectId),
  );
  const [worktrees, setWorktrees] = useState<WorktreeRow[]>([]);
  const [branch, setBranch] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const rows = await worktreeList({ project_id: projectId });
      setWorktrees(rows);
    } catch (e) {
      setError(formatErr(e));
    }
  }, [projectId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

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
      await refresh();
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

  const onRemove = async (w: WorktreeRow) => {
    if (
      !window.confirm(
        t("project_settings_modal.remove_confirm", { name: w.branch }),
      )
    )
      return;
    try {
      await worktreeRemove({ id: w.id });
      await refresh();
    } catch (e) {
      setError(formatErr(e));
    }
  };

  return (
    <div className="flex max-h-[80vh] w-[640px] max-w-[90vw] flex-col overflow-hidden rounded-xl bg-neutral-950">
      <header className="flex items-center justify-between border-b border-neutral-800 px-5 py-3">
        <h2 className="text-sm font-medium text-neutral-100">
          {t("project_settings_modal.title", {
            name: project?.name ?? "—",
          })}
        </h2>
      </header>

      <div className="flex-1 overflow-y-auto px-5 py-4">
        <section>
          <div className="mb-1.5 flex items-center justify-between">
            <h3 className="text-[12px] font-medium uppercase tracking-wider text-neutral-400">
              {t("project_settings_modal.worktrees_heading")}
            </h3>
          </div>
          <p className="mb-3 text-[11px] text-neutral-500">
            {t("project_settings_modal.worktrees_help")}
          </p>

          <ul className="flex flex-col gap-1.5">
            {worktrees.map((w) => (
              <li
                key={w.id}
                className="flex items-center gap-2 rounded-md border border-neutral-800 bg-neutral-900/40 px-3 py-2"
              >
                {w.is_primary ? (
                  <Star
                    className="size-3.5 shrink-0 text-amber-300"
                    strokeWidth={1.75}
                  />
                ) : (
                  <GitBranch
                    className="size-3.5 shrink-0 text-neutral-400"
                    strokeWidth={1.75}
                  />
                )}
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm text-neutral-100">
                    {w.branch}
                  </div>
                  <div className="truncate text-[10px] text-neutral-500">
                    {w.path}
                  </div>
                </div>
                {!w.is_primary && (
                  <button
                    type="button"
                    onClick={() => void onRemove(w)}
                    aria-label="remove"
                    className="flex size-6 items-center justify-center rounded text-neutral-500 hover:bg-red-950/40 hover:text-red-300"
                  >
                    <X className="size-3.5" strokeWidth={1.75} />
                  </button>
                )}
              </li>
            ))}
          </ul>

          <div className="mt-4 flex gap-2">
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
              placeholder={t(
                "project_settings_modal.new_worktree_prompt",
              ).replace(/:$/, "")}
              disabled={creating}
              className="flex-1 rounded-md border border-neutral-800 bg-neutral-900 px-2.5 py-1.5 text-sm text-neutral-100 placeholder:text-neutral-500 outline-none focus:border-neutral-700 disabled:opacity-50"
            />
            <button
              type="button"
              onClick={() => void onCreate()}
              disabled={!branch.trim() || creating}
              className="inline-flex items-center gap-1 rounded-md bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-900 transition hover:bg-white disabled:cursor-not-allowed disabled:bg-neutral-800 disabled:text-neutral-500"
            >
              <Plus className="size-3.5" strokeWidth={2} />
              {creating
                ? t("empty_state.creating")
                : t("empty_state.new_worktree_submit")}
            </button>
          </div>
          {error && (
            <p className="mt-2 rounded border border-red-900/40 bg-red-950/20 px-2.5 py-1.5 text-[11px] text-red-200">
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
