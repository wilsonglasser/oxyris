import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  GitBranch,
  Image as ImageIcon,
  Plus,
  Sparkles,
  Star,
  Trash2,
  X,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  projectAutodetectLogo,
  projectDelete,
  projectRename,
  projectSetLogo,
  projectSetWorkspace,
} from "~/ipc/commands.ts";
import {
  type WorktreeRow,
  WorktreeCommandError,
  worktreeCreate,
  worktreeList,
  worktreeRemove,
} from "~/ipc/worktree.ts";
import { runAutoActionsOnWorktreeCreate } from "~/lib/runAutoActions.ts";
import { useProjectStore, workspacesOf } from "~/stores/projectStore.ts";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";

interface Props {
  projectId: string;
  onClose?: () => void;
}

/**
 * Project-scoped settings modal. Today: just the worktree manager moved out
 * of the sidebar. Future home for project rename, default model, runtime
 * mode, etc.
 */
export function ProjectSettingsModal({ projectId, onClose }: Props) {
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
        <NameSection projectId={projectId} />

        <LogoSection projectId={projectId} />

        <WorkspaceSection projectId={projectId} />

        <section className="mt-6">
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

        <DangerSection projectId={projectId} onDeleted={onClose} />
      </div>
    </div>
  );
}

function DangerSection({
  projectId,
  onDeleted,
}: {
  projectId: string;
  onDeleted?: (() => void) | undefined;
}) {
  const { t } = useTranslation("common");
  const project = useProjectStore((s) =>
    s.projects.find((p) => p.id === projectId),
  );
  const refreshProjects = useProjectStore((s) => s.refresh);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!project) return null;

  const onDelete = async () => {
    if (busy) return;
    if (
      !window.confirm(
        t("project_settings_modal.delete_confirm", { name: project.name }),
      )
    )
      return;
    setBusy(true);
    setError(null);
    try {
      await projectDelete({ id: projectId });
      await refreshProjects();
      onDeleted?.();
    } catch (e) {
      setError(formatErr(e));
      setBusy(false);
    }
  };

  return (
    <section className="mt-6 border-t border-neutral-800 pt-5">
      <h3 className="mb-1.5 text-[12px] font-medium uppercase tracking-wider text-red-400">
        {t("project_settings_modal.danger_heading")}
      </h3>
      <p className="mb-3 text-[11px] text-neutral-500">
        {t("project_settings_modal.delete_help")}
      </p>
      <button
        type="button"
        onClick={() => void onDelete()}
        disabled={busy}
        className="inline-flex items-center gap-1.5 rounded-md border border-red-900/50 bg-red-950/20 px-3 py-1.5 text-sm font-medium text-red-300 transition enabled:hover:bg-red-900/30 disabled:cursor-not-allowed disabled:opacity-40"
      >
        <Trash2 className="size-3.5" strokeWidth={2} />
        {t("project_settings_modal.delete_label")}
      </button>
      {error && (
        <p className="mt-2 rounded border border-red-900/40 bg-red-950/20 px-2.5 py-1.5 text-[11px] text-red-200">
          {error}
        </p>
      )}
    </section>
  );
}

function formatErr(e: unknown): string {
  if (e instanceof WorktreeCommandError) {
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}

function NameSection({ projectId }: { projectId: string }) {
  const { t } = useTranslation("common");
  const project = useProjectStore((s) =>
    s.projects.find((p) => p.id === projectId),
  );
  const refreshProjects = useProjectStore((s) => s.refresh);
  const [value, setValue] = useState(project?.name ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!project) return null;

  const trimmed = value.trim();
  const dirty = trimmed !== project.name && trimmed.length > 0;

  const commit = async () => {
    if (!dirty || busy) return;
    setBusy(true);
    setError(null);
    try {
      await projectRename({ id: projectId, new_name: trimmed });
      await refreshProjects();
    } catch (e) {
      setError(formatErr(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="mb-6">
      <h3 className="mb-1.5 text-[12px] font-medium uppercase tracking-wider text-neutral-400">
        {t("project_settings_modal.name_heading")}
      </h3>
      <div className="flex gap-2">
        <input
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void commit();
            }
          }}
          placeholder={t("project_settings_modal.name_placeholder")}
          disabled={busy}
          className="flex-1 rounded-md border border-neutral-800 bg-neutral-900 px-2.5 py-1.5 text-sm text-neutral-100 placeholder:text-neutral-500 outline-none focus:border-neutral-700 disabled:opacity-50"
        />
        <button
          type="button"
          onClick={() => void commit()}
          disabled={!dirty || busy}
          className="rounded-md bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-900 transition hover:bg-white disabled:cursor-not-allowed disabled:bg-neutral-800 disabled:text-neutral-500"
        >
          {t("project_settings_modal.name_save")}
        </button>
      </div>
      {error && (
        <p className="mt-2 rounded border border-red-900/40 bg-red-950/20 px-2.5 py-1.5 text-[11px] text-red-200">
          {error}
        </p>
      )}
    </section>
  );
}

function WorkspaceSection({ projectId }: { projectId: string }) {
  const { t } = useTranslation("common");
  const project = useProjectStore((s) =>
    s.projects.find((p) => p.id === projectId),
  );
  const projects = useProjectStore((s) => s.projects);
  const refreshProjects = useProjectStore((s) => s.refresh);
  const known = workspacesOf(projects);
  const [value, setValue] = useState(project?.workspace ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!project) return null;

  const current = project.workspace ?? "";
  const dirty = value.trim() !== current;

  const commit = async () => {
    if (!dirty || busy) return;
    setBusy(true);
    setError(null);
    try {
      await projectSetWorkspace({
        id: projectId,
        workspace: value.trim() || null,
      });
      await refreshProjects();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="mt-6">
      <h3 className="mb-1.5 text-[12px] font-medium uppercase tracking-wider text-neutral-400">
        {t("project_settings_modal.workspace_heading")}
      </h3>
      <p className="mb-3 text-[11px] text-neutral-500">
        {t("project_settings_modal.workspace_help")}
      </p>
      <div className="flex gap-2">
        <input
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void commit();
            }
          }}
          list="oxyris-workspaces-settings"
          placeholder={t("project_settings_modal.workspace_placeholder")}
          disabled={busy}
          className="flex-1 rounded-md border border-neutral-800 bg-neutral-900 px-2.5 py-1.5 text-sm text-neutral-100 placeholder:text-neutral-500 outline-none focus:border-neutral-700 disabled:opacity-50"
        />
        <datalist id="oxyris-workspaces-settings">
          {known.map((ws) => (
            <option key={ws} value={ws} />
          ))}
        </datalist>
        <button
          type="button"
          onClick={() => void commit()}
          disabled={!dirty || busy}
          className="rounded-md bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-900 transition hover:bg-white disabled:cursor-not-allowed disabled:bg-neutral-800 disabled:text-neutral-500"
        >
          {t("project_settings_modal.workspace_save")}
        </button>
      </div>
      {error && (
        <p className="mt-2 rounded border border-red-900/40 bg-red-950/20 px-2.5 py-1.5 text-[11px] text-red-200">
          {error}
        </p>
      )}
    </section>
  );
}

function LogoSection({ projectId }: { projectId: string }) {
  const { t } = useTranslation("common");
  const project = useProjectStore((s) =>
    s.projects.find((p) => p.id === projectId),
  );
  const refreshProjects = useProjectStore((s) => s.refresh);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!project) return null;

  const apply = async (logoPath: string | null) => {
    setBusy(true);
    setError(null);
    try {
      await projectSetLogo({ id: projectId, logo_path: logoPath });
      await refreshProjects();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onPickFile = async () => {
    setError(null);
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        filters: [
          {
            name: "Images",
            extensions: ["png", "jpg", "jpeg", "webp", "svg", "ico", "gif"],
          },
        ],
      });
      if (typeof picked === "string") {
        await apply(picked);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const onAutodetect = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await projectAutodetectLogo({ id: projectId });
      if (!res.logo_path) {
        setError(t("project_settings_modal.logo_autodetect_empty"));
      } else {
        await apply(res.logo_path);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section>
      <h3 className="mb-1.5 text-[12px] font-medium uppercase tracking-wider text-neutral-400">
        {t("project_settings_modal.logo_heading")}
      </h3>
      <p className="mb-3 text-[11px] text-neutral-500">
        {t("project_settings_modal.logo_help")}
      </p>
      <div className="flex items-center gap-3">
        <ProjectBadge
          name={project.name}
          projectId={projectId}
          logoPath={project.logo_path}
          size={48}
        />
        <div className="flex flex-col gap-1.5">
          <div className="flex flex-wrap gap-1.5">
            <button
              type="button"
              onClick={() => void onPickFile()}
              disabled={busy}
              className="inline-flex items-center gap-1 rounded-md border border-neutral-800 bg-neutral-900 px-2.5 py-1 text-[11px] text-neutral-200 enabled:hover:bg-neutral-800 disabled:opacity-40"
            >
              <ImageIcon className="size-3" strokeWidth={2} />
              {t("project_settings_modal.logo_pick")}
            </button>
            <button
              type="button"
              onClick={() => void onAutodetect()}
              disabled={busy}
              className="inline-flex items-center gap-1 rounded-md border border-neutral-800 bg-neutral-900 px-2.5 py-1 text-[11px] text-neutral-200 enabled:hover:bg-neutral-800 disabled:opacity-40"
            >
              <Sparkles className="size-3 text-amber-300" strokeWidth={2} />
              {t("project_settings_modal.logo_autodetect")}
            </button>
            {project.logo_path && (
              <button
                type="button"
                onClick={() => void apply(null)}
                disabled={busy}
                className="inline-flex items-center gap-1 rounded-md border border-red-900/50 bg-red-950/20 px-2.5 py-1 text-[11px] text-red-300 enabled:hover:bg-red-900/30 disabled:opacity-40"
              >
                <Trash2 className="size-3" strokeWidth={2} />
                {t("project_settings_modal.logo_clear")}
              </button>
            )}
          </div>
          {project.logo_path && (
            <code className="text-[10px] text-neutral-500">
              {project.logo_path}
            </code>
          )}
          {error && (
            <p className="text-[10px] text-red-300">{error}</p>
          )}
        </div>
      </div>
    </section>
  );
}
