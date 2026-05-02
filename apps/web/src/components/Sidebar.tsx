import { useCallback, useEffect, useMemo, useState } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  GitBranch,
  MessageSquarePlus,
  Pencil,
  Plus,
  Search,
  Settings,
  Star,
  Trash2,
  X,
} from "lucide-react";
import {
  sessionDelete,
  sessionList,
  sessionRename,
  type SessionSummary,
} from "~/ipc/session.ts";
import {
  type WorktreeRow,
  WorktreeCommandError,
  worktreeCreate,
  worktreeList,
  worktreeRemove,
} from "~/ipc/worktree.ts";
import { useProjectStore } from "~/stores/projectStore.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { useHasUpdate } from "~/stores/updaterStore.ts";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";
import { runAutoActionsOnWorktreeCreate } from "~/lib/runAutoActions.ts";

interface Props {
  onNewProject: () => void;
  onOpenSettings: () => void;
  onNewSession?: () => void;
}

type SessionsByProject = Record<string, SessionSummary[]>;

export function Sidebar({ onNewProject, onOpenSettings, onNewSession }: Props) {
  const { t } = useTranslation("common");
  const projects = useProjectStore((s) => s.projects);
  const activeProjectId = useProjectStore((s) => s.activeId);
  const setActiveProject = useProjectStore((s) => s.setActive);
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const setActiveSession = useSessionStore((s) => s.setActive);

  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [sessionsByProject, setSessionsByProject] = useState<SessionsByProject>(
    {},
  );
  const hasUpdate = useHasUpdate();

  const refreshProjectSessions = useCallback(async (projectId: string) => {
    try {
      const rows = await sessionList({ project_id: projectId });
      setSessionsByProject((prev) => ({ ...prev, [projectId]: rows }));
    } catch {
      /* leave prior snapshot in place */
    }
  }, []);

  // Keep session lists in sync with the known projects. Fetch for every
  // project so search can match by thread title even for collapsed nodes.
  useEffect(() => {
    projects.forEach((p) => void refreshProjectSessions(p.id));
    setSessionsByProject((prev) => {
      // Drop cached rows for projects that no longer exist so stale data
      // doesn't stick around.
      const allowed = new Set(projects.map((p) => p.id));
      const next: SessionsByProject = {};
      for (const [pid, rows] of Object.entries(prev)) {
        if (allowed.has(pid)) next[pid] = rows;
      }
      return next;
    });
  }, [projects, refreshProjectSessions]);

  // Refresh the active project's sessions when the active session id
  // changes — catches create/delete without waiting for a focus refresh.
  useEffect(() => {
    if (!activeProjectId) return;
    void refreshProjectSessions(activeProjectId);
  }, [activeProjectId, activeSessionId, refreshProjectSessions]);

  // Auto-expand the active project so the user always sees its threads.
  useEffect(() => {
    if (activeProjectId) {
      setExpanded((prev) =>
        prev[activeProjectId] ? prev : { ...prev, [activeProjectId]: true },
      );
    }
  }, [activeProjectId]);

  const q = query.trim().toLowerCase();
  const searching = q.length > 0;

  const visibleProjects = useMemo(() => {
    if (!searching) return projects;
    return projects.filter((p) => {
      if (p.name.toLowerCase().includes(q)) return true;
      const rows = sessionsByProject[p.id] ?? [];
      return rows.some((row) =>
        (row.title ?? row.model ?? "").toLowerCase().includes(q),
      );
    });
  }, [projects, sessionsByProject, q, searching]);

  const toggle = (id: string) =>
    setExpanded((prev) => ({ ...prev, [id]: !prev[id] }));

  const visibleSessionsFor = (
    projectId: string,
    projectName: string,
  ): SessionSummary[] => {
    const all = sessionsByProject[projectId] ?? [];
    if (!searching) return all;
    const projectMatches = projectName.toLowerCase().includes(q);
    if (projectMatches) return all;
    return all.filter((s) =>
      (s.title ?? s.model ?? "").toLowerCase().includes(q),
    );
  };

  return (
    <aside className="flex h-full w-64 shrink-0 flex-col border-r border-neutral-800 bg-neutral-900 text-neutral-300">
      <div className="flex items-center gap-2 px-3 py-2.5">
        <div className="relative flex-1">
          <Search
            className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-neutral-500"
            strokeWidth={1.75}
          />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("sidebar.search_placeholder")}
            className="w-full rounded-md border border-neutral-800 bg-neutral-900/60 py-1 pl-7 pr-2 text-[11px] text-neutral-200 placeholder:text-neutral-500 outline-none focus:border-neutral-700"
          />
        </div>
        <button
          type="button"
          onClick={onNewProject}
          aria-label={t("sidebar.new_project")}
          title={t("sidebar.new_project")}
          className="flex size-7 shrink-0 items-center justify-center rounded-md border border-neutral-800 text-neutral-300 hover:bg-neutral-800 hover:text-neutral-100"
        >
          <Plus className="size-3.5" strokeWidth={1.75} />
        </button>
      </div>

      <div className="px-3 pb-1">
        <span className="text-[9px] font-medium uppercase tracking-wider text-neutral-500">
          {t("sidebar.projects")}
        </span>
      </div>

      <div className="flex-1 overflow-y-auto px-2 pb-2">
        {visibleProjects.length === 0 ? (
          <p className="px-2 py-1.5 text-[11px] text-neutral-500">
            {searching ? t("sidebar.no_results") : t("sidebar.no_projects")}
          </p>
        ) : (
          <ul className="flex flex-col gap-0.5">
            {visibleProjects.map((p) => (
              <ProjectItem
                key={p.id}
                project={p}
                isActive={p.id === activeProjectId}
                isExpanded={searching || !!expanded[p.id]}
                onToggle={() => toggle(p.id)}
                onSelectProject={() => {
                  setActiveProject(p.id);
                  setExpanded((prev) => ({ ...prev, [p.id]: true }));
                }}
                activeSessionId={activeSessionId}
                onSelectSession={(id) => {
                  setActiveProject(p.id);
                  setActiveSession(id);
                }}
                onNewSession={onNewSession}
                sessions={visibleSessionsFor(p.id, p.name)}
                onSessionsChanged={() => void refreshProjectSessions(p.id)}
              />
            ))}
          </ul>
        )}
      </div>

      <footer className="flex items-center justify-between border-t border-neutral-800 px-2 py-1.5">
        <button
          type="button"
          onClick={onOpenSettings}
          aria-label={
            hasUpdate
              ? t("sidebar.update_available")
              : t("sidebar.open_settings")
          }
          title={
            hasUpdate
              ? t("sidebar.update_available")
              : t("sidebar.open_settings")
          }
          className="relative flex size-7 items-center justify-center rounded-md text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
        >
          <Settings className="size-3.5" strokeWidth={1.75} />
          {hasUpdate && (
            <span className="absolute right-1 top-1 size-1.5 rounded-full bg-emerald-400" />
          )}
        </button>
        <span className="pr-1.5 text-[10px] text-neutral-600">v0.1.0</span>
      </footer>
    </aside>
  );
}

interface ProjectItemProps {
  project: import("~/ipc/commands.ts").ProjectRow;
  isActive: boolean;
  isExpanded: boolean;
  onToggle: () => void;
  onSelectProject: () => void;
  activeSessionId: string | null;
  onSelectSession: (id: string | null) => void;
  onNewSession?: (() => void) | undefined;
  sessions: SessionSummary[];
  onSessionsChanged: () => void;
}

function ProjectItem({
  project,
  isActive,
  isExpanded,
  onToggle,
  onSelectProject,
  activeSessionId,
  onSelectSession,
  onNewSession,
  sessions,
  onSessionsChanged,
}: ProjectItemProps) {
  const { t } = useTranslation("common");
  const [worktrees, setWorktrees] = useState<WorktreeRow[]>([]);
  const [worktreesOpen, setWorktreesOpen] = useState(false);
  const [wtError, setWtError] = useState<string | null>(null);

  const refreshWorktrees = useCallback(() => {
    void worktreeList({ project_id: project.id })
      .then(setWorktrees)
      .catch(() => {});
  }, [project.id]);

  useEffect(() => {
    if (!isExpanded || !worktreesOpen) return;
    refreshWorktrees();
  }, [isExpanded, worktreesOpen, refreshWorktrees]);

  const onNewWorktree = async () => {
    const branch = window.prompt(
      t("sidebar.new_worktree_prompt", { project: project.name }),
    );
    if (!branch) return;
    try {
      const created = await worktreeCreate({ project_id: project.id, branch });
      refreshWorktrees();
      setWtError(null);
      void runAutoActionsOnWorktreeCreate({
        projectId: project.id,
        worktreeId: created.id,
        sessionId: activeSessionId,
      });
    } catch (err) {
      setWtError(formatWorktreeError(err, t));
    }
  };

  const onRemoveWorktree = async (w: WorktreeRow) => {
    if (!window.confirm(t("sidebar.remove_worktree_confirm", { name: w.name })))
      return;
    try {
      await worktreeRemove({ id: w.id });
      refreshWorktrees();
    } catch (err) {
      setWtError(formatWorktreeError(err, t));
    }
  };

  const envLabel =
    project.environment.kind === "windows"
      ? "Windows"
      : `WSL · ${project.environment.distro}`;

  return (
    <li className="flex flex-col">
      <div
        className={`group flex items-center gap-1 rounded-md pr-1 transition ${
          isActive
            ? "bg-neutral-800/70 text-neutral-100"
            : "text-neutral-300 hover:bg-neutral-800/40"
        }`}
      >
        <button
          type="button"
          onClick={onToggle}
          aria-label={isExpanded ? "collapse" : "expand"}
          className="flex size-5 shrink-0 items-center justify-center text-neutral-500 hover:text-neutral-200"
        >
          {isExpanded ? (
            <ChevronDown className="size-3" strokeWidth={2} />
          ) : (
            <ChevronRight className="size-3" strokeWidth={2} />
          )}
        </button>
        <button
          type="button"
          onClick={onSelectProject}
          className="flex min-w-0 flex-1 items-center gap-2 py-1 text-left"
        >
          <ProjectBadge name={project.name} size={18} />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[12px] font-medium">
              {project.name}
            </span>
            <span className="block truncate text-[10px] text-neutral-500">
              {envLabel}
            </span>
          </span>
        </button>
        {onNewSession && isActive && (
          <button
            type="button"
            onClick={onNewSession}
            aria-label={t("sidebar.new_thread")}
            title={t("sidebar.new_thread")}
            className="flex size-5 items-center justify-center rounded text-neutral-500 opacity-0 transition hover:bg-neutral-700 hover:text-neutral-100 group-hover:opacity-100"
          >
            <MessageSquarePlus className="size-3" strokeWidth={1.75} />
          </button>
        )}
      </div>

      {isExpanded && (
        <div className="ml-4 mt-0.5 flex flex-col gap-1.5 border-l border-neutral-800/80 pl-2">
          <ul className="flex flex-col gap-0.5">
            {sessions.length === 0 ? (
              <li className="px-1.5 py-1 text-[10px] text-neutral-600">
                {t("sidebar.no_sessions")}
              </li>
            ) : (
              sessions.map((s) => (
                <SessionEntry
                  key={s.id}
                  session={s}
                  isActive={s.id === activeSessionId}
                  onSelect={() => onSelectSession(s.id)}
                  onRenamed={onSessionsChanged}
                  onDeleted={() => {
                    if (activeSessionId === s.id) onSelectSession(null);
                    onSessionsChanged();
                  }}
                />
              ))
            )}
          </ul>

          <div>
            <button
              type="button"
              onClick={() => setWorktreesOpen((v) => !v)}
              className="flex w-full items-center gap-1 rounded px-1.5 py-1 text-[10px] uppercase tracking-wider text-neutral-500 hover:bg-neutral-800/40 hover:text-neutral-300"
            >
              {worktreesOpen ? (
                <ChevronDown className="size-3" strokeWidth={2} />
              ) : (
                <ChevronRight className="size-3" strokeWidth={2} />
              )}
              <GitBranch className="size-3" strokeWidth={1.75} />
              <span className="flex-1 text-left">
                {t("sidebar.worktrees")}
              </span>
              <span className="text-neutral-600">
                {worktrees.length || ""}
              </span>
            </button>
            {worktreesOpen && (
              <ul className="ml-1 mt-0.5 flex flex-col gap-0.5">
                <li className="flex justify-end px-1">
                  <button
                    type="button"
                    onClick={() => void onNewWorktree()}
                    className="inline-flex items-center gap-1 rounded border border-neutral-800 px-1.5 py-0.5 text-[9px] text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
                  >
                    <Plus className="size-2.5" strokeWidth={2} />
                    {t("sidebar.new_worktree")}
                  </button>
                </li>
                {wtError && (
                  <li className="rounded border border-red-900/60 bg-red-950/30 px-1.5 py-1 text-[10px] text-red-200">
                    {wtError}
                  </li>
                )}
                {worktrees.length === 0 ? (
                  <li className="px-1.5 py-0.5 text-[10px] text-neutral-600">
                    {t("sidebar.no_worktrees")}
                  </li>
                ) : (
                  worktrees.map((w) => (
                    <li
                      key={w.id}
                      className="group flex items-center gap-1.5 rounded px-1.5 py-0.5 text-[11px] text-neutral-400 hover:bg-neutral-800/40"
                      title={w.path}
                    >
                      {w.is_primary ? (
                        <Star
                          className="size-3 shrink-0 text-neutral-500"
                          strokeWidth={1.75}
                        />
                      ) : (
                        <GitBranch
                          className="size-3 shrink-0 text-neutral-600"
                          strokeWidth={1.75}
                        />
                      )}
                      <span className="min-w-0 flex-1 truncate">
                        {w.branch}
                      </span>
                      {!w.is_primary && (
                        <button
                          type="button"
                          onClick={() => void onRemoveWorktree(w)}
                          aria-label="remove worktree"
                          className="flex size-4 items-center justify-center rounded text-neutral-500 opacity-0 transition hover:bg-red-950/40 hover:text-red-300 group-hover:opacity-100"
                        >
                          <X className="size-2.5" strokeWidth={2} />
                        </button>
                      )}
                    </li>
                  ))
                )}
              </ul>
            )}
          </div>
        </div>
      )}
    </li>
  );
}

interface SessionEntryProps {
  session: SessionSummary;
  isActive: boolean;
  onSelect: () => void;
  onRenamed: () => void;
  onDeleted: () => void;
}

function SessionEntry({
  session,
  isActive,
  onSelect,
  onRenamed,
  onDeleted,
}: SessionEntryProps) {
  const { t } = useTranslation("common");
  const label = session.title || session.model || t("sidebar.untitled_session");

  const onRename = async (e: React.MouseEvent) => {
    e.stopPropagation();
    const next = window.prompt(
      t("sidebar.rename_session_prompt"),
      session.title || "",
    );
    if (next === null) return;
    const trimmed = next.trim();
    if (!trimmed || trimmed === session.title) return;
    try {
      await sessionRename({ session_id: session.id, title: trimmed });
      onRenamed();
    } catch {
      /* keep whatever the store eventually reconciles via event */
    }
  };

  const onDelete = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!window.confirm(t("sidebar.delete_session_confirm", { name: label })))
      return;
    try {
      await sessionDelete({ session_id: session.id });
      onDeleted();
    } catch {
      /* noop */
    }
  };

  return (
    <li>
      <div
        className={`group flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-[11px] transition ${
          isActive
            ? "bg-[#2e436e]/40 text-neutral-100"
            : "text-neutral-400 hover:bg-neutral-800/40"
        }`}
      >
        <StatusDot status={session.status} />
        <button
          type="button"
          onClick={onSelect}
          onDoubleClick={(e) => void onRename(e)}
          className="min-w-0 flex-1 text-left"
        >
          <span className="block truncate">{label}</span>
          <span className="block truncate text-[9px] text-neutral-500">
            {session.turn_count} · {formatRelative(session.last_activity_at)}
          </span>
        </button>
        <div className="flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100">
          <button
            type="button"
            onClick={(e) => void onRename(e)}
            aria-label={t("sidebar.rename_session")}
            title={t("sidebar.rename_session")}
            className="flex size-4 items-center justify-center rounded text-neutral-500 hover:bg-neutral-700 hover:text-neutral-200"
          >
            <Pencil className="size-2.5" strokeWidth={1.75} />
          </button>
          <button
            type="button"
            onClick={(e) => void onDelete(e)}
            aria-label={t("sidebar.delete_session")}
            title={t("sidebar.delete_session")}
            className="flex size-4 items-center justify-center rounded text-neutral-500 hover:bg-red-950/40 hover:text-red-300"
          >
            <Trash2 className="size-2.5" strokeWidth={1.75} />
          </button>
        </div>
      </div>
    </li>
  );
}

function StatusDot({ status }: { status: string }) {
  const color =
    status === "running"
      ? "bg-emerald-400"
      : status === "errored"
        ? "bg-red-400"
        : "bg-neutral-600";
  return (
    <span
      className={`inline-block size-1.5 shrink-0 rounded-full ${color}`}
    />
  );
}

function formatWorktreeError(err: unknown, t: TFunction<"common">): string {
  if (err instanceof WorktreeCommandError) {
    switch (err.tauri.code) {
      case "domain":
        return err.tauri.message;
      case "git":
        return t("sidebar.worktree_error.git", { message: err.tauri.message });
      case "storage":
        return t("sidebar.worktree_error.storage", { message: err.tauri.message });
      case "project_not_found":
        return t("sidebar.worktree_error.project_not_found");
      case "projection":
        return t("sidebar.worktree_error.projection", { message: err.tauri.message });
      case "empty_repo":
        return t("sidebar.worktree_error.empty_repo");
    }
  }
  return err instanceof Error ? err.message : t("sidebar.worktree_error.unknown");
}

function formatRelative(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const diff = Date.now() - then;
  const m = Math.floor(diff / 60000);
  if (m < 1) return "now";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d`;
  return new Date(iso).toLocaleDateString();
}
