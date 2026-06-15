import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getVersion } from "@tauri-apps/api/app";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  GitBranch,
  MessageSquarePlus,
  Pencil,
  Pin,
  PinOff,
  Plus,
  Search,
  Settings,
  Trash2,
} from "lucide-react";
import {
  onSessionApproval,
  onSessionEvent,
  sessionDelete,
  sessionList,
  sessionRename,
  sessionTogglePin,
  type SessionSummary,
} from "~/ipc/session.ts";
import {
  PRIMARY_WORKTREE_ID,
  type WorktreeRow,
  worktreeList,
} from "~/ipc/worktree.ts";
import {
  ALL_WORKSPACES,
  useProjectStore,
  workspacesOf,
} from "~/stores/projectStore.ts";
import { projectReorder, type ProjectRow } from "~/ipc/commands.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { useBusyStore } from "~/stores/busyStore.ts";
import { useAutopilotStore } from "~/stores/autopilotStore.ts";
import { useHasUpdate } from "~/stores/updaterStore.ts";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";
import {
  playCompletionChime,
  playEscalationChime,
  playInputChime,
  shouldNotify,
} from "~/lib/notificationSound.ts";
import { useAutopilotAlertStore } from "~/stores/autopilotAlertStore.ts";
import { bumpBadge } from "~/lib/taskbarBadge.ts";
import { localEnvLabel } from "~/lib/host.ts";
import {
  claudePureState,
  onPureState,
  type PureState,
} from "~/ipc/terminal.ts";
import { onAutopilotEvent } from "~/ipc/autopilot.ts";

interface Props {
  onNewProject: () => void;
  onOpenSettings: () => void;
  onNewSession?: (project: import("~/ipc/commands.ts").ProjectRow) => void;
  onOpenProjectSettings?: (projectId: string) => void;
}

type SessionsByProject = Record<string, SessionSummary[]>;
type WorktreesByProject = Record<string, WorktreeRow[]>;

export function Sidebar({
  onNewProject,
  onOpenSettings,
  onNewSession,
  onOpenProjectSettings,
}: Props) {
  const { t } = useTranslation("common");
  const projects = useProjectStore((s) => s.projects);
  const activeProjectId = useProjectStore((s) => s.activeId);
  const setActiveProject = useProjectStore((s) => s.setActive);
  const workspaceFilter = useProjectStore((s) => s.workspaceFilter);
  const setWorkspaceFilter = useProjectStore((s) => s.setWorkspaceFilter);
  const expanded = useProjectStore((s) => s.expanded);
  const toggleExpanded = useProjectStore((s) => s.toggleExpanded);
  const setExpanded = useProjectStore((s) => s.setExpanded);
  const activeSessionId = useSessionStore((s) => s.activeSessionId);
  const setActiveSession = useSessionStore((s) => s.setActive);
  const markAttention = useSessionStore((s) => s.markAttention);
  const setNeedsInput = useSessionStore((s) => s.setNeedsInput);
  const setBusy = useBusyStore((s) => s.setBusy);

  const [query, setQuery] = useState("");
  const [sessionsByProject, setSessionsByProject] = useState<SessionsByProject>(
    {},
  );
  const [worktreesByProject, setWorktreesByProject] =
    useState<WorktreesByProject>({});
  // Drag-to-reorder state. `dragId` is the project being dragged; `dropTarget`
  // is the row the cursor is currently over plus an insertion side. Both are
  // cleared on dragend so an aborted drag leaves no UI residue.
  const [dragId, setDragId] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<{
    id: string;
    pos: "before" | "after";
  } | null>(null);
  const refreshProjects = useProjectStore((s) => s.refresh);
  const hasUpdate = useHasUpdate();

  // App version read from the bundle at runtime (Tauri injects it from
  // tauri.conf.json), so the footer always matches the installed build.
  const [version, setVersion] = useState<string>("");
  useEffect(() => {
    void getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  // User-resizable sidebar width, persisted across reloads. Bounds keep the
  // chrome usable.
  const [width, setWidth] = useState<number>(() => readStoredWidth());
  useEffect(() => {
    try {
      window.localStorage.setItem(SIDEBAR_WIDTH_KEY, String(width));
    } catch {
      /* localStorage may be disabled in odd contexts */
    }
  }, [width]);
  const dragStartRef = useRef<{ x: number; w: number } | null>(null);
  const onResizeStart = (e: React.MouseEvent) => {
    e.preventDefault();
    dragStartRef.current = { x: e.clientX, w: width };
    const onMove = (ev: MouseEvent) => {
      const start = dragStartRef.current;
      if (!start) return;
      const next = Math.max(
        SIDEBAR_MIN,
        Math.min(SIDEBAR_MAX, start.w + (ev.clientX - start.x)),
      );
      setWidth(next);
    };
    const onUp = () => {
      dragStartRef.current = null;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "col-resize";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const refreshProjectSessions = useCallback(async (projectId: string) => {
    try {
      const rows = await sessionList({ project_id: projectId });
      setSessionsByProject((prev) => ({ ...prev, [projectId]: rows }));
    } catch {
      /* leave prior snapshot in place */
    }
  }, []);

  const refreshProjectWorktrees = useCallback(async (projectId: string) => {
    try {
      const rows = await worktreeList({ project_id: projectId });
      setWorktreesByProject((prev) => ({ ...prev, [projectId]: rows }));
    } catch {
      /* leave prior snapshot in place */
    }
  }, []);

  // Keep session lists in sync with the known projects. Fetch for every
  // project so search can match by thread title even for collapsed nodes.
  useEffect(() => {
    projects.forEach((p) => {
      void refreshProjectSessions(p.id);
      void refreshProjectWorktrees(p.id);
    });
    setSessionsByProject((prev) => filterAllowed(prev, projects));
    setWorktreesByProject((prev) => filterAllowed(prev, projects));
  }, [projects, refreshProjectSessions, refreshProjectWorktrees]);

  // Refresh the active project's sessions when the active session id
  // changes — catches create/delete without waiting for a focus refresh.
  useEffect(() => {
    if (!activeProjectId) return;
    void refreshProjectSessions(activeProjectId);
  }, [activeProjectId, activeSessionId, refreshProjectSessions]);

  // The active thread's events stream to ChatPanel, not here, so a rename
  // (notably the auto-generated title after the first turn) never refreshes
  // the sidebar's cached summaries on its own. Listen for it directly.
  useEffect(() => {
    if (!activeSessionId || !activeProjectId) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void onSessionEvent(activeSessionId, (payload) => {
      if (payload.event.kind === "SessionRenamed") {
        void refreshProjectSessions(activeProjectId);
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [activeSessionId, activeProjectId, refreshProjectSessions]);

  // Flag background threads that finish while the user is elsewhere. Session
  // events arrive on a per-session channel and only the *active* thread is
  // subscribed in ChatPanel, so we listen here for every running, non-active
  // thread: when its turn reaches a terminal outcome (or it errors) we mark it
  // for attention. Opening the thread clears the flag (see store `setActive`).
  const allSessions = useMemo(
    () => Object.values(sessionsByProject).flat(),
    [sessionsByProject],
  );
  const sessionsRef = useRef<SessionSummary[]>([]);
  sessionsRef.current = allSessions;
  // Re-subscribe only when the watched set actually changes (not on every poll).
  const watchKey = useMemo(
    () =>
      allSessions
        .filter((s) => s.status === "running" && s.id !== activeSessionId)
        .map((s) => s.id)
        .sort()
        .join(","),
    [allSessions, activeSessionId],
  );
  useEffect(() => {
    if (!watchKey) return;
    const ids = new Set(watchKey.split(","));
    const targets = sessionsRef.current.filter((s) => ids.has(s.id));
    const unlistens: Array<() => void> = [];
    let cancelled = false;
    for (const s of targets) {
      void onSessionEvent(s.id, (payload) => {
        const kind = payload.event.kind;
        // A rename (e.g. auto-title) only needs the cached summary refreshed,
        // not an attention flag.
        if (kind === "SessionRenamed") {
          void refreshProjectSessions(s.project_id);
          return;
        }
        if (kind === "TurnStarted") {
          // Working again → blue. Any prior "wants input" (red) is moot.
          setBusy(s.id, true);
          setNeedsInput(s.id, false);
          return;
        }
        if (
          kind === "TurnCompleted" ||
          kind === "TurnFailed" ||
          kind === "TurnInterrupted" ||
          kind === "SessionErrored"
        ) {
          // blue → orange: chime only when this thread was actually working
          // and the window is unfocused (the user can't see the bull).
          const wasBusy = useBusyStore.getState().busy[s.id];
          setBusy(s.id, false);
          setNeedsInput(s.id, false);
          markAttention(s.id);
          void refreshProjectSessions(s.project_id);
          if (wasBusy && shouldNotify()) {
            playCompletionChime();
            bumpBadge();
          }
        }
      }).then((fn) => {
        if (cancelled) fn();
        else unlistens.push(fn);
      });
      // A pending tool-approval is the strongest "needs you" signal → red bull.
      // blue → red: chime when backgrounded so the user knows to come decide.
      void onSessionApproval(s.id, () => {
        setNeedsInput(s.id, true);
        if (shouldNotify()) {
          playInputChime();
          bumpBadge();
        }
      }).then((fn) => {
        if (cancelled) fn();
        else unlistens.push(fn);
      });
      // Pure threads emit none of the structured events above — their dot is
      // driven separately by the pure-state bridge effect below (the sole owner
      // of pure dot state).
      // Keep the sidebar's robot glyph honest for backgrounded pilots: when the
      // backend halts/errors a thread we're not viewing, clear its engaged flag
      // here (PureSessionView only listens for the active session).
      void onAutopilotEvent(s.id, (event) => {
        const store = useAutopilotStore.getState();
        store.setThinking(s.id, event.kind === "thinking");
        if (
          event.kind === "halted" ||
          event.kind === "error" ||
          event.kind === "escalated"
        ) {
          store.setEnabled(s.id, false);
        }
        if (event.kind === "escalated") {
          playEscalationChime();
          useAutopilotAlertStore.getState().raise(s.id, event.why);
        }
      }).then((fn) => {
        if (cancelled) fn();
        else unlistens.push(fn);
      });
    }
    return () => {
      cancelled = true;
      for (const fn of unlistens) fn();
    };
  }, [watchKey, markAttention, setNeedsInput, setBusy, refreshProjectSessions]);

  // ── Pure-mode dot bridge ────────────────────────────────────────────────
  // The SINGLE owner of pure (claude TUI) dot state. The backend emits a LEVEL
  // snapshot per session (`session:<id>:pure-state`) on every change and on a
  // heartbeat; we diff it against the previous snapshot to derive the chime /
  // attention transitions. One effect covers BOTH the active pure thread and
  // background ones — the structured-event loop above deliberately skips the
  // active and non-running sessions, so without this they'd have no driver.
  // Replaces the three racing `onPureSignal` listeners (App, PureClaudePanel,
  // and the loop above) that each wrote these stores and disagreed on the end
  // colour. Watches every session id (the backend only emits for claude PTYs,
  // so a structured thread simply never fires).
  const pureWatchKey = useMemo(
    () =>
      allSessions
        .map((s) => s.id)
        .sort()
        .join(","),
    [allSessions],
  );
  const purePrevRef = useRef<Map<string, PureState>>(new Map());
  // Pending busy→idle settle timers, keyed by session. The backend emits
  // transient settle snapshots (busy=false) mid-turn between spinner frames; a
  // raw apply would flap the dot blue→orange on a background thread and
  // blue→green on the active one. We hold blue until the thread stays settled
  // for SETTLE_DEBOUNCE_MS before treating the turn as done.
  const settleTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(
    new Map(),
  );
  useEffect(() => {
    if (!pureWatchKey) return;
    const ids = new Set(pureWatchKey.split(","));
    // Drop prev-snapshot state for sessions no longer watched (deleted / not in
    // the set), so a re-subscribe (set change, StrictMode remount) can't derive
    // a transition against a stale prev and drop/phantom a chime.
    for (const id of purePrevRef.current.keys()) {
      if (!ids.has(id)) purePrevRef.current.delete(id);
    }
    const targets = sessionsRef.current.filter((s) => ids.has(s.id));
    const unlistens: Array<() => void> = [];
    let cancelled = false;
    // Apply a snapshot: set the dot stores, then ring/flag on transitions.
    // `seed` snapshots (the `claudePureState` reconcile) only prime the dot —
    // they never chime and never stamp activity, so opening the app can't
    // green-dot or ring an idle thread.
    const apply = (s: SessionSummary, snap: PureState, seed: boolean) => {
      const prev = purePrevRef.current.get(s.id);
      purePrevRef.current.set(s.id, snap);
      // `needs_input` (red) outranks `busy` (blue) at the dot — urgent, never
      // debounced.
      setNeedsInput(s.id, snap.needs_input);

      const clearSettle = () => {
        const pending = settleTimersRef.current.get(s.id);
        if (pending) {
          clearTimeout(pending);
          settleTimersRef.current.delete(s.id);
        }
      };

      // Anything is happening → read blue right away and cancel any pending
      // settle (the turn is still alive — that idle frame was transient).
      if (snap.busy || snap.needs_input) {
        clearSettle();
        setBusy(s.id, snap.busy && !snap.needs_input);
        if (seed) return;
        // Stamp recency only while something is actually happening. A settle
        // snapshot (both false) isn't fresh activity, so it mustn't keep an
        // idle thread reading green-recent.
        useSessionStore.getState().touchActivity(s.id);
        // idle/blue → red: a prompt just appeared. Ring so a backgrounded user
        // knows to come decide.
        if (prev && !prev.needs_input && snap.needs_input && shouldNotify()) {
          playInputChime();
          bumpBadge();
        }
        return;
      }

      // Settle snapshot (busy=false, needs_input=false). A seed just reconciles
      // the dot to idle (never chimes/flags). For a live snapshot we debounce:
      // hold blue until the thread stays settled, so a transient mid-turn idle
      // frame can't flap the dot blue→orange (background) or blue→green
      // (active).
      if (seed) {
        clearSettle();
        setBusy(s.id, false);
        return;
      }
      const wasBusy = !!useBusyStore.getState().busy[s.id];
      if (!wasBusy) {
        // Already idle — nothing to settle, no completion to announce.
        setBusy(s.id, false);
        return;
      }
      if (settleTimersRef.current.has(s.id)) return; // settle already pending
      const timer = setTimeout(() => {
        settleTimersRef.current.delete(s.id);
        // A newer snapshot may have re-armed the thread while we waited.
        const latest = purePrevRef.current.get(s.id);
        if (latest && (latest.busy || latest.needs_input)) return;
        // blue → done: a turn finished without a waiting prompt. Flag for
        // attention (no-op on the active thread) and chime when backgrounded.
        setBusy(s.id, false);
        markAttention(s.id);
        void refreshProjectSessions(s.project_id);
        if (shouldNotify()) {
          playCompletionChime();
          bumpBadge();
        }
      }, SETTLE_DEBOUNCE_MS);
      settleTimersRef.current.set(s.id, timer);
    };
    for (const s of targets) {
      void onPureState(s.id, (snap) => apply(s, snap, false)).then((fn) => {
        if (cancelled) fn();
        else unlistens.push(fn);
      });
      // Seed from backend ground truth so a snapshot the listener missed (it
      // registered after the backend emitted the current state) is corrected
      // immediately, rather than waiting for the next heartbeat.
      void claudePureState({ session_id: s.id })
        .then((snap) => {
          if (!cancelled) apply(s, snap, true);
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
      for (const fn of unlistens) fn();
      for (const t of settleTimersRef.current.values()) clearTimeout(t);
      settleTimersRef.current.clear();
    };
  }, [
    pureWatchKey,
    markAttention,
    setNeedsInput,
    setBusy,
    refreshProjectSessions,
  ]);

  // Auto-expand the active project so the user always sees its threads —
  // but only when it is the sole project. With multiple projects every group
  // stays collapsed on open; the user expands the one they want.
  useEffect(() => {
    if (activeProjectId && projects.length === 1) {
      setExpanded(activeProjectId, true);
    }
  }, [activeProjectId, projects.length, setExpanded]);

  const q = query.trim().toLowerCase();
  const searching = q.length > 0;

  const workspaces = useMemo(() => workspacesOf(projects), [projects]);

  // Apply the workspace filter first, then the search filter. The empty
  // string is never a stored label, so a stale filter (workspace deleted)
  // simply yields no projects until the user switches back to "All".
  const inWorkspace = useMemo(() => {
    if (workspaceFilter === ALL_WORKSPACES) return projects;
    return projects.filter((p) => (p.workspace ?? "") === workspaceFilter);
  }, [projects, workspaceFilter]);

  const visibleProjects = useMemo(() => {
    if (!searching) return inWorkspace;
    return inWorkspace.filter((p) => {
      if (p.name.toLowerCase().includes(q)) return true;
      const rows = sessionsByProject[p.id] ?? [];
      return rows.some((row) =>
        (row.title ?? row.model ?? "").toLowerCase().includes(q),
      );
    });
  }, [inWorkspace, sessionsByProject, q, searching]);

  const toggle = (id: string) => toggleExpanded(id);

  // Drag-to-reorder. The dragged project's new `sort_order` is the midpoint
  // between its visible-list neighbors after the drop, so one drop = one event
  // (no whole-list renumber). At the edges we pick `neighbor ± 1.0` so the
  // value stays finite. Drops onto the dragged row itself, or that would land
  // in the same slot, are no-ops.
  const reorderTo = useCallback(
    async (movedId: string, overId: string, pos: "before" | "after") => {
      if (movedId === overId) return;
      const filtered = visibleProjects.filter((p) => p.id !== movedId);
      const overIdx = filtered.findIndex((p) => p.id === overId);
      if (overIdx < 0) return;
      const insertIdx = pos === "before" ? overIdx : overIdx + 1;
      const prev = filtered[insertIdx - 1];
      const next = filtered[insertIdx];
      let sortOrder: number;
      if (prev && next) sortOrder = (prev.sort_order + next.sort_order) / 2;
      else if (prev) sortOrder = prev.sort_order + 1;
      else if (next) sortOrder = next.sort_order - 1;
      else return;
      // No-op when the row already sits in that slot (sort_order unchanged).
      const moved = visibleProjects.find((p) => p.id === movedId);
      if (moved && moved.sort_order === sortOrder) return;
      try {
        await projectReorder({ id: movedId, sort_order: sortOrder });
        await refreshProjects();
      } catch {
        // Backend rejected (e.g. project gone). The next refresh will reconcile.
      }
    },
    [visibleProjects, refreshProjects],
  );

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
    <aside
      style={{ width }}
      className="relative flex h-full shrink-0 flex-col border-r border-neutral-800 bg-neutral-900 text-neutral-300"
    >
      <div className="flex items-center gap-1 px-2 py-2">
        <div className="relative flex-1">
          <Search
            className="pointer-events-none absolute left-1.5 top-1/2 size-3 -translate-y-1/2 text-neutral-500"
            strokeWidth={1.75}
          />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("sidebar.search_placeholder")}
            className="h-6 w-full rounded bg-neutral-900/60 pl-6 pr-2 text-[11px] text-neutral-200 placeholder:text-neutral-600 outline-none focus:bg-neutral-800/60"
          />
        </div>
        <button
          type="button"
          onClick={onNewProject}
          aria-label={t("sidebar.new_project")}
          title={t("sidebar.new_project")}
          className="flex size-6 shrink-0 items-center justify-center rounded text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
        >
          <Plus className="size-3.5" strokeWidth={1.75} />
        </button>
      </div>

      <div className="flex items-center justify-between gap-2 px-3 pb-1">
        <span className="text-[9px] font-medium uppercase tracking-wider text-neutral-500">
          {t("sidebar.projects")}
        </span>
        {workspaces.length > 0 && (
          <select
            value={workspaceFilter}
            onChange={(e) => setWorkspaceFilter(e.target.value)}
            aria-label={t("sidebar.workspace_filter")}
            title={t("sidebar.workspace_filter")}
            className="max-w-[55%] truncate rounded bg-transparent py-0.5 text-[10px] text-neutral-400 outline-none hover:text-neutral-200 focus:text-neutral-200"
          >
            <option value={ALL_WORKSPACES} className="bg-neutral-900">
              {t("sidebar.workspace_all")}
            </option>
            {workspaces.map((ws) => (
              <option key={ws} value={ws} className="bg-neutral-900">
                {ws}
              </option>
            ))}
          </select>
        )}
      </div>

      <div className="flex-1 overflow-y-auto px-2 pb-2">
        {visibleProjects.length === 0 ? (
          <p className="px-2 py-1.5 text-[11px] text-neutral-500">
            {searching ? t("sidebar.no_results") : t("sidebar.no_projects")}
          </p>
        ) : (
          <ul className="flex flex-col gap-0.5">
            {visibleProjects.map((p) => {
              const worktrees = worktreesByProject[p.id] ?? [];
              const worktreeNameById: Record<string, string> = {};
              for (const w of worktrees) {
                worktreeNameById[w.id] = w.is_primary
                  ? `${w.branch || "main"} ★`
                  : w.branch;
              }
              // Sessions persisted before the synthetic-primary landed
              // carry `worktree_id: null` for the root checkout. Map that
              // to the same label the synthetic primary card shows.
              const primaryLabel = worktreeNameById[PRIMARY_WORKTREE_ID];
              return (
                <ProjectItem
                  key={p.id}
                  project={p}
                  isActive={p.id === activeProjectId}
                  isExpanded={searching || !!expanded[p.id]}
                  onToggle={() => toggle(p.id)}
                  onSelectProject={() => {
                    setActiveProject(p.id);
                    setActiveSession(null);
                    setExpanded(p.id, true);
                  }}
                  activeSessionId={activeSessionId}
                  onSelectSession={(id) => {
                    setActiveProject(p.id);
                    setActiveSession(id);
                  }}
                  onNewSession={
                    onNewSession
                      ? () => {
                          setActiveProject(p.id);
                          onNewSession(p);
                        }
                      : undefined
                  }
                  onOpenProjectSettings={
                    onOpenProjectSettings
                      ? () => onOpenProjectSettings(p.id)
                      : undefined
                  }
                  sessions={visibleSessionsFor(p.id, p.name)}
                  worktreeNameById={worktreeNameById}
                  primaryLabel={primaryLabel ?? null}
                  onSessionsChanged={() => void refreshProjectSessions(p.id)}
                  isDragging={dragId === p.id}
                  dropIndicator={
                    dropTarget && dropTarget.id === p.id && dragId !== p.id
                      ? dropTarget.pos
                      : null
                  }
                  // Reorder is disabled inside search results — the list is no
                  // longer a stable sortable view, so a drop would land
                  // somewhere the user can't see.
                  dragEnabled={!searching}
                  onDragStartItem={() => setDragId(p.id)}
                  onDragOverItem={(pos) => {
                    if (!dragId || dragId === p.id) return;
                    setDropTarget((prev) =>
                      prev && prev.id === p.id && prev.pos === pos
                        ? prev
                        : { id: p.id, pos },
                    );
                  }}
                  onDragLeaveItem={() => {
                    setDropTarget((prev) =>
                      prev && prev.id === p.id ? null : prev,
                    );
                  }}
                  onDropItem={(pos) => {
                    const moved = dragId;
                    setDragId(null);
                    setDropTarget(null);
                    if (moved) void reorderTo(moved, p.id, pos);
                  }}
                  onDragEndItem={() => {
                    setDragId(null);
                    setDropTarget(null);
                  }}
                />
              );
            })}
          </ul>
        )}
      </div>

      <footer className="flex items-center justify-between border-t border-neutral-800 px-2 py-1">
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
          className="relative flex size-6 items-center justify-center rounded text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
        >
          <Settings className="size-3.5" strokeWidth={1.75} />
          {hasUpdate && (
            <span className="absolute right-0.5 top-0.5 size-1.5 rounded-full bg-emerald-400" />
          )}
        </button>
        {version && (
          <span className="pr-1 text-[10px] text-neutral-600">v{version}</span>
        )}
      </footer>

      {/*
        Drag handle on the right edge — 4px wide invisible strip with a
        visible bar on hover. Lives outside the scroll container so dragging
        stays smooth even mid-scroll.
      */}
      <div
        onMouseDown={onResizeStart}
        role="separator"
        aria-orientation="vertical"
        className="group absolute right-0 top-0 z-10 h-full w-1 cursor-col-resize"
      >
        <div className="h-full w-full bg-transparent transition group-hover:bg-emerald-700/50" />
      </div>
    </aside>
  );
}

const SIDEBAR_WIDTH_KEY = "oxyris.sidebar.width";
const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 480;
const SIDEBAR_DEFAULT = 240;

function readStoredWidth(): number {
  try {
    const raw = window.localStorage.getItem(SIDEBAR_WIDTH_KEY);
    const n = Number(raw);
    if (Number.isFinite(n) && n >= SIDEBAR_MIN && n <= SIDEBAR_MAX) {
      return n;
    }
  } catch {
    /* fall through */
  }
  return SIDEBAR_DEFAULT;
}

interface ProjectItemProps {
  project: ProjectRow;
  isActive: boolean;
  isExpanded: boolean;
  onToggle: () => void;
  onSelectProject: () => void;
  activeSessionId: string | null;
  onSelectSession: (id: string | null) => void;
  onNewSession?: (() => void) | undefined;
  onOpenProjectSettings?: (() => void) | undefined;
  sessions: SessionSummary[];
  worktreeNameById: Record<string, string>;
  primaryLabel: string | null;
  onSessionsChanged: () => void;
  /** This row is the one currently being dragged. Dims its content. */
  isDragging: boolean;
  /**
   * `"before"` / `"after"` when the cursor is over this row while another
   * project is being dragged; the corresponding edge gets a visible line.
   * `null` otherwise.
   */
  dropIndicator: "before" | "after" | null;
  /** False disables `draggable` and reorder handlers (e.g. while searching). */
  dragEnabled: boolean;
  onDragStartItem: () => void;
  onDragOverItem: (pos: "before" | "after") => void;
  onDragLeaveItem: () => void;
  onDropItem: (pos: "before" | "after") => void;
  onDragEndItem: () => void;
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
  onOpenProjectSettings,
  sessions,
  worktreeNameById,
  primaryLabel,
  onSessionsChanged,
  isDragging,
  dropIndicator,
  dragEnabled,
  onDragStartItem,
  onDragOverItem,
  onDragLeaveItem,
  onDropItem,
  onDragEndItem,
}: ProjectItemProps) {
  const { t } = useTranslation("common");

  // Strongest session signal in this project, so a collapsed row still shows
  // when something inside wants the user. Mirrors StatusDot's priority:
  // red (needs input / errored) > blue (working) > orange (done, unseen) >
  // green (recently active).
  const attnTier = useSessionStore((st) => {
    let tier = 0;
    for (const s of sessions) {
      if (st.needsInput[s.id] || s.status === "errored") return 4;
      if (st.attention[s.id]) tier = Math.max(tier, 2);
    }
    return tier;
  });
  const busyTier = useBusyStore((st) =>
    sessions.some((s) => st.busy[s.id]) ? 3 : 0,
  );
  const recentTier = sessions.some((s) => isRecent(s.last_activity_at)) ? 1 : 0;
  const tier = Math.max(attnTier, busyTier, recentTier);
  const tierClass =
    tier === 4
      ? "bg-red-500/15 text-red-100 hover:bg-red-500/25"
      : tier === 3
        ? "bg-sky-500/10 text-sky-100 hover:bg-sky-500/20"
        : tier === 2
          ? "bg-orange-500/15 text-orange-100 hover:bg-orange-500/25"
          : tier === 1
            ? "bg-emerald-500/10 text-emerald-100 hover:bg-emerald-500/20"
            : null;

  const envLabel =
    project.environment.kind === "local"
      ? localEnvLabel()
      : `WSL · ${project.environment.distro}`;

  // Split the row vertically at its midpoint to decide "before" vs "after"
  // when the cursor hovers — drops on the top half insert above, bottom
  // half below. Native HTML5 DnD gives us clientY; the row's rect gives us
  // its bounds.
  const sideFromEvent = (
    e: React.DragEvent<HTMLLIElement>,
  ): "before" | "after" => {
    const rect = e.currentTarget.getBoundingClientRect();
    return e.clientY < rect.top + rect.height / 2 ? "before" : "after";
  };

  return (
    <li
      draggable={dragEnabled}
      onDragStart={(e) => {
        if (!dragEnabled) return;
        // Some browsers refuse to start a drag unless setData is called.
        e.dataTransfer.effectAllowed = "move";
        try {
          e.dataTransfer.setData("text/plain", project.id);
        } catch {
          /* setData can throw in some sandboxed contexts; the drag still works */
        }
        onDragStartItem();
      }}
      onDragOver={(e) => {
        if (!dragEnabled) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        onDragOverItem(sideFromEvent(e));
      }}
      onDragLeave={onDragLeaveItem}
      onDrop={(e) => {
        if (!dragEnabled) return;
        e.preventDefault();
        onDropItem(sideFromEvent(e));
      }}
      onDragEnd={onDragEndItem}
      className={`flex flex-col ${isDragging ? "opacity-50" : ""} ${
        dropIndicator === "before"
          ? "border-t border-emerald-400"
          : dropIndicator === "after"
            ? "border-b border-emerald-400"
            : ""
      }`}
    >
      <div
        className={`group flex items-center gap-1 rounded-md pr-1 transition ${
          isActive
            ? "bg-neutral-800/70 text-neutral-100"
            : tierClass ?? "text-neutral-300 hover:bg-neutral-800/40"
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
          <ProjectBadge
            name={project.name}
            projectId={project.id}
            logoPath={project.logo_path}
            size={18}
          />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[12px] font-medium">
              {project.name}
            </span>
            <span className="block truncate text-[10px] text-neutral-500">
              {envLabel}
            </span>
          </span>
        </button>
        {onOpenProjectSettings && isActive && (
          <button
            type="button"
            onClick={onOpenProjectSettings}
            aria-label={t("sidebar.project_settings")}
            title={t("sidebar.project_settings")}
            className="flex size-5 items-center justify-center rounded text-neutral-500 opacity-0 transition hover:bg-neutral-700 hover:text-neutral-100 group-hover:opacity-100"
          >
            <Settings className="size-3" strokeWidth={1.75} />
          </button>
        )}
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
                  worktreeName={
                    !s.worktree_id || s.worktree_id === PRIMARY_WORKTREE_ID
                      ? primaryLabel
                      : worktreeNameById[s.worktree_id] ?? primaryLabel
                  }
                  onSelect={() => onSelectSession(s.id)}
                  onChanged={onSessionsChanged}
                  onDeleted={() => {
                    if (activeSessionId === s.id) onSelectSession(null);
                    onSessionsChanged();
                  }}
                />
              ))
            )}
          </ul>
        </div>
      )}
    </li>
  );
}

interface SessionEntryProps {
  session: SessionSummary;
  isActive: boolean;
  worktreeName: string | null;
  onSelect: () => void;
  onChanged: () => void;
  onDeleted: () => void;
}

function SessionEntry({
  session,
  isActive,
  worktreeName,
  onSelect,
  onChanged,
  onDeleted,
}: SessionEntryProps) {
  const { t } = useTranslation("common");
  const label = session.title || session.model || t("sidebar.untitled_session");
  const pinned = !!session.pinned_at;
  // Orange highlight when this background thread finished/errored and the user
  // hasn't opened it yet. Active threads are being viewed, so never flagged.
  const needsAttention = useSessionStore((s) => !!s.attention[session.id]);
  // Red — Claude is paused waiting on a tool-approval input.
  const needsInput = useSessionStore((s) => !!s.needsInput[session.id]);
  // Live PTY activity (pure threads have no Turn event to bump
  // last_activity_at). Feeds StatusDot's green "recent" window.
  const liveActivityAt = useSessionStore((s) => s.liveActivity[session.id]);
  const busy = useBusyStore((s) => !!s.busy[session.id]);
  // Auto-pilot engaged → the status dot becomes a robot (same color meaning),
  // so a thread being driven hands-free is recognizable from any tab.
  const autopilot = useAutopilotStore((s) => !!s.enabled[session.id]);
  const autopilotThinking = useAutopilotStore((s) => !!s.thinking[session.id]);

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
      onChanged();
    } catch {
      /* keep whatever the store eventually reconciles via event */
    }
  };

  const onTogglePin = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await sessionTogglePin({ session_id: session.id });
      onChanged();
    } catch {
      /* event will reconcile */
    }
  };

  const onDelete = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!window.confirm(t("sidebar.delete_session_confirm", { name: label })))
      return;
    try {
      await sessionDelete({ session_id: session.id });
      // Forget all per-session UI state so a deleted thread can't leave a
      // stranded dot (busy/needsInput/attention/liveActivity) behind.
      useSessionStore.getState().drop(session.id);
      useBusyStore.getState().drop(session.id);
      onDeleted();
    } catch {
      /* noop */
    }
  };

  const subtitle = (() => {
    const parts: string[] = [];
    if (worktreeName) parts.push(worktreeName);
    parts.push(formatRelative(session.last_activity_at));
    return parts.join(" · ");
  })();

  return (
    <li>
      <div
        className={`group relative flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-[11px] transition ${
          isActive
            ? "bg-[#2e436e]/40 text-neutral-100"
            : needsInput
              ? "bg-red-500/15 text-red-100 hover:bg-red-500/25"
              : needsAttention
                ? "bg-orange-500/20 text-orange-100 hover:bg-orange-500/30"
                : "text-neutral-400 hover:bg-neutral-800/40"
        }`}
      >
        <StatusDot
          status={session.status}
          attention={needsAttention}
          needsInput={needsInput}
          busy={busy}
          autopilot={autopilot}
          autopilotThinking={autopilotThinking}
          lastActivityAt={session.last_activity_at}
          liveActivityAt={liveActivityAt}
        />
        <button
          type="button"
          onClick={onSelect}
          onDoubleClick={(e) => void onRename(e)}
          className="min-w-0 flex-1 text-left"
        >
          <span className="block truncate">{label}</span>
          <span className="block truncate text-[9px] text-neutral-500">
            <GitBranch
              className="-mt-px mr-0.5 inline size-2.5 align-middle"
              strokeWidth={1.75}
            />
            {subtitle}
          </span>
        </button>
        <div className="absolute right-1.5 top-1/2 flex -translate-y-1/2 items-center gap-0.5 pl-3">
          <button
            type="button"
            onClick={(e) => void onTogglePin(e)}
            aria-label={
              pinned ? t("sidebar.unpin_session") : t("sidebar.pin_session")
            }
            title={
              pinned ? t("sidebar.unpin_session") : t("sidebar.pin_session")
            }
            className={`flex size-4 items-center justify-center rounded transition ${
              pinned
                ? "text-amber-300 hover:bg-amber-950/40"
                : "text-neutral-500 opacity-0 hover:bg-neutral-700 hover:text-neutral-200 group-hover:opacity-100"
            }`}
          >
            {pinned ? (
              <Pin className="size-2.5" strokeWidth={2} />
            ) : (
              <PinOff className="size-2.5" strokeWidth={1.75} />
            )}
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
      </div>
    </li>
  );
}

// "Talked recently" window for the green bull — activity within the last hour.
const RECENT_MS = 60 * 60 * 1000;

// How long a pure thread must stay settled (busy=false, needs_input=false)
// before we treat the turn as done. Wider than the TUI spinner/settle cadence
// so a transient idle frame mid-turn doesn't flap the dot blue→orange/green.
const SETTLE_DEBOUNCE_MS = 1500;

function isRecent(iso: string): boolean {
  const t = new Date(iso).getTime();
  return Number.isFinite(t) && Date.now() - t < RECENT_MS;
}

/**
 * The bull. Four meanings, highest priority first:
 *   red    — Claude wants you to pick an input (paused on approval), or errored.
 *   blue   — working: a turn is in flight (pulses).
 *   orange — Claude finished a background thread you haven't opened yet.
 *   green  — talked recently (within the last hour).
 *   gray   — idle / stale.
 */
function StatusDot({
  status,
  attention,
  needsInput,
  busy,
  autopilot,
  autopilotThinking,
  lastActivityAt,
  liveActivityAt,
}: {
  status: string;
  attention?: boolean;
  needsInput?: boolean;
  busy?: boolean;
  /** Auto-pilot engaged → render a robot glyph in the dot's color, not a dot. */
  autopilot?: boolean;
  /** Pilot is mid-step (consulting the Supervisor) → spin the robot. */
  autopilotThinking?: boolean;
  lastActivityAt: string;
  /** Wall-clock ms of last live PTY output (pure threads); overrides recency. */
  liveActivityAt?: number | undefined;
}) {
  // Resolve the single color the dot would have, highest priority first. The
  // robot variant reuses these exact colors so the meaning is unchanged.
  const recent =
    isRecent(lastActivityAt) ||
    (liveActivityAt !== undefined && Date.now() - liveActivityAt < RECENT_MS);
  // Literal class strings (not built via concat) so Tailwind's JIT emits them.
  const red = needsInput || status === "errored";
  const dotBg = red
    ? "bg-red-500"
    : busy
      ? "bg-sky-400"
      : attention
        ? "bg-orange-400"
        : recent
          ? "bg-emerald-400"
          : "bg-neutral-600";

  if (autopilot) {
    // Same color, robot shape. Pulses while a turn is in flight (mirrors the
    // dot's blue ping) so an actively-driven thread still reads as "working".
    const text = red
      ? "text-red-500"
      : busy
        ? "text-sky-400"
        : attention
          ? "text-orange-400"
          : recent
            ? "text-emerald-400"
            : "text-neutral-600";
    return (
      <Bot
        className={`size-3 shrink-0 ${text} ${
          autopilotThinking ? "animate-spin" : busy ? "animate-pulse" : ""
        }`}
        strokeWidth={2}
      />
    );
  }
  // A turn in flight → blue, pulsing, so "working" reads at a glance.
  if (busy) {
    return (
      <span className="relative inline-flex size-1.5 shrink-0">
        <span className="absolute inline-flex size-full animate-ping rounded-full bg-sky-400 opacity-75" />
        <span className="relative inline-flex size-1.5 rounded-full bg-sky-400" />
      </span>
    );
  }
  return (
    <span className={`inline-block size-1.5 shrink-0 rounded-full ${dotBg}`} />
  );
}

function filterAllowed<T>(
  prev: Record<string, T>,
  projects: { id: string }[],
): Record<string, T> {
  const allowed = new Set(projects.map((p) => p.id));
  const next: Record<string, T> = {};
  for (const [pid, rows] of Object.entries(prev)) {
    if (allowed.has(pid)) next[pid] = rows;
  }
  return next;
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
