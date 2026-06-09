import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  GripVertical,
  PanelLeft,
  PanelLeftClose,
  Plus,
  Send,
  Sparkles,
  Terminal as TerminalIcon,
  X,
} from "lucide-react";
import type { ProjectRow } from "~/ipc/commands.ts";
import {
  type SessionKind,
  type SessionSummary,
  sessionGet,
  sessionList,
  sessionSendMessage,
  sessionStart,
} from "~/ipc/session.ts";
import { terminalWrite } from "~/ipc/terminal.ts";
import { useProjectStore } from "~/stores/projectStore.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import {
  type MvCols,
  gridColsClass,
  maxPanes,
  useMultiViewStore,
} from "~/stores/multiViewStore.ts";
import { ChatPanel } from "~/components/ChatPanel.tsx";
import { NewChatModal } from "~/components/NewChatModal.tsx";
import { ProjectBadge } from "~/components/ProjectBadge.tsx";
import { PureClaudePanel } from "~/components/PureClaudePanel.tsx";
import { TerminalPanel } from "~/components/TerminalPanel.tsx";

interface PaneInfo {
  sessionId: string;
  kind: SessionKind | null;
  ptyId: string | null;
}

type SessionsByProject = Record<string, SessionSummary[]>;

/**
 * Multi View — a grid (cols × up to 3 rows) where each pane embeds an existing
 * session (chat or pure). Panes are multi-project: each pane's session picker
 * groups every project's threads under an <optgroup>, so a single grid can mix
 * sessions from different projects. A broadcast bar fans the same prompt to
 * every pane. Layout persists via the multiView store.
 */
export function MultiViewPanel() {
  const { t } = useTranslation("chat");
  const projects = useProjectStore((s) => s.projects);
  // Global "Claude Code puro" toggle. App.tsx renders the pure PTY purely off
  // this flag (ignoring stored kind), so Multi must mirror it — otherwise a
  // session created before kind was persisted (stored "structured" but shown
  // pure everywhere else) embeds the empty ChatPanel here.
  const pureMode = useAppSettingsStore((s) => s.pureMode);

  const panes = useMultiViewStore((s) => s.panes);
  const cols = useMultiViewStore((s) => s.cols);
  const addPane = useMultiViewStore((s) => s.addPane);
  const removePane = useMultiViewStore((s) => s.removePane);
  const setPaneSession = useMultiViewStore((s) => s.setPaneSession);
  const setCols = useMultiViewStore((s) => s.setCols);
  const sidebarHidden = useMultiViewStore((s) => s.sidebarHidden);
  const toggleSidebar = useMultiViewStore((s) => s.toggleSidebar);
  const setPanes = useMultiViewStore((s) => s.setPanes);
  const movePane = useMultiViewStore((s) => s.movePane);

  const [sessionsByProject, setSessionsByProject] = useState<SessionsByProject>(
    {},
  );
  const [broadcast, setBroadcast] = useState("");
  const broadcastRef = useRef<HTMLTextAreaElement | null>(null);
  // The terminal dock belongs to the Multi View (not a grid pane); it follows
  // whichever pane the user last focused.
  const [focusedPaneId, setFocusedPaneId] = useState<string | null>(null);
  const [terminalOpen, setTerminalOpen] = useState(false);
  // Pane awaiting a project pick for its "new session" action.
  const [newPaneId, setNewPaneId] = useState<string | null>(null);
  // Pane currently being dragged by its handle (logo), for reordering.
  const [dragPaneId, setDragPaneId] = useState<string | null>(null);

  const focusedPane =
    panes.find((p) => p.paneId === focusedPaneId) ?? panes[0];
  const focusedSessionId = focusedPane?.sessionId ?? null;

  // Live registry of what each pane resolved to (kind + pty id), keyed by
  // paneId. A ref because broadcast only reads it on submit — no re-render.
  const infoRef = useRef<Map<string, PaneInfo>>(new Map());

  const refreshSessions = useCallback(async () => {
    const entries = await Promise.all(
      projects.map(async (p) => {
        try {
          return [p.id, await sessionList({ project_id: p.id })] as const;
        } catch {
          return [p.id, [] as SessionSummary[]] as const;
        }
      }),
    );
    setSessionsByProject(Object.fromEntries(entries));
  }, [projects]);

  useEffect(() => {
    void refreshSessions();
  }, [refreshSessions, panes]);

  // sessionId → which project it belongs to, so each pane can embed the panel
  // with the right project context regardless of the global active project.
  const projectBySession = useMemo(() => {
    const m = new Map<string, ProjectRow>();
    for (const p of projects) {
      for (const s of sessionsByProject[p.id] ?? []) m.set(s.id, p);
    }
    return m;
  }, [projects, sessionsByProject]);

  const allSessions = useMemo(() => {
    const out: SessionSummary[] = [];
    for (const p of projects) out.push(...(sessionsByProject[p.id] ?? []));
    return out;
  }, [projects, sessionsByProject]);

  const reportInfo = useCallback((paneId: string, info: PaneInfo) => {
    infoRef.current.set(paneId, info);
  }, []);

  const doBroadcast = useCallback(() => {
    const text = broadcast.trim();
    if (!text) return;
    for (const pane of panes) {
      if (!pane.sessionId) continue;
      const info = infoRef.current.get(pane.paneId);
      if (info?.kind === "pure" && info.ptyId) {
        void terminalWrite({ id: info.ptyId, data: `${text}\r` }).catch(() => {});
      } else {
        void sessionSendMessage({ session_id: pane.sessionId, text }).catch(
          () => {},
        );
      }
    }
    setBroadcast("");
  }, [broadcast, panes]);

  // Auto-grow the broadcast composer with content, capped (max-h-32 = 128px),
  // and collapse back to one row when cleared (broadcast / empty).
  useEffect(() => {
    const el = broadcastRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 128)}px`;
  }, [broadcast]);

  // Autofill: drop every project's sessions into the grid (running first, then
  // most recent), capped at the current column count × 3 rows.
  const autofill = useCallback(() => {
    const ordered = [...allSessions].sort((a, b) => {
      const ar = a.status === "running" ? 0 : 1;
      const br = b.status === "running" ? 0 : 1;
      if (ar !== br) return ar - br;
      return b.last_activity_at.localeCompare(a.last_activity_at);
    });
    setPanes(ordered.map((s) => s.id));
  }, [allSessions, setPanes]);

  // Project picked for a pane's "new session" → spin up a pure session in it
  // and bind the pane to it.
  const onNewPanePick = useCallback(
    async (project: ProjectRow) => {
      const paneId = newPaneId;
      setNewPaneId(null);
      if (!paneId) return;
      try {
        const res = await sessionStart({
          project_id: project.id,
          provider_id: "claude",
          environment: project.environment,
          cwd: project.root_path,
          model: "",
          runtime: "supervised",
          env_mode: "default",
          kind: "pure",
        });
        setPaneSession(paneId, res.session_id);
        void refreshSessions();
      } catch {
        /* ignore — surfaced nowhere critical for v1 */
      }
    },
    [newPaneId, setPaneSession, refreshSessions],
  );

  if (projects.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-neutral-500">
        {t("mv_no_projects")}
      </div>
    );
  }

  const atMax = panes.length >= maxPanes(cols);

  return (
    <section className="flex h-full min-h-0 flex-col bg-neutral-950">
      <header className="flex items-center justify-between gap-2 border-b border-neutral-800 bg-neutral-900 px-3 py-1.5">
        <span className="truncate text-[11px] font-medium text-neutral-200">
          {t("mv_title")}
        </span>
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            onClick={toggleSidebar}
            title={sidebarHidden ? t("mv_show_sidebar") : t("mv_hide_sidebar")}
            aria-label={
              sidebarHidden ? t("mv_show_sidebar") : t("mv_hide_sidebar")
            }
            className={`inline-flex items-center gap-1 rounded border px-2 py-1 text-[11px] ${
              sidebarHidden
                ? "border-neutral-700 text-neutral-300 hover:bg-neutral-800"
                : "border-neutral-600 bg-neutral-800 text-neutral-100"
            }`}
          >
            {sidebarHidden ? (
              <PanelLeft className="size-3" strokeWidth={1.75} />
            ) : (
              <PanelLeftClose className="size-3" strokeWidth={1.75} />
            )}
          </button>
          <button
            type="button"
            onClick={() => setTerminalOpen((v) => !v)}
            disabled={!focusedSessionId}
            title={t("mv_terminal")}
            aria-label={t("mv_terminal")}
            className={`inline-flex items-center gap-1 rounded border px-2 py-1 text-[11px] disabled:opacity-50 ${
              terminalOpen
                ? "border-neutral-600 bg-neutral-800 text-neutral-100"
                : "border-neutral-700 text-neutral-300 hover:bg-neutral-800"
            }`}
          >
            <TerminalIcon className="size-3" strokeWidth={1.75} />
            {t("mv_terminal")}
          </button>
          <label className="flex items-center gap-1 text-[11px] text-neutral-400">
            {t("mv_columns")}
            <select
              value={cols}
              onChange={(e) => setCols(Number(e.target.value) as MvCols)}
              className="rounded border border-neutral-700 bg-neutral-950 px-1.5 py-0.5 text-[11px] text-neutral-200 outline-none focus:border-neutral-600"
            >
              {[2, 3, 4, 5].map((c) => (
                <option key={c} value={c} className="bg-neutral-900">
                  {c}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            onClick={autofill}
            disabled={allSessions.length === 0}
            className="inline-flex items-center gap-1 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-50"
          >
            <Sparkles className="size-3" strokeWidth={1.75} />
            {t("mv_autofill")}
          </button>
          <button
            type="button"
            onClick={addPane}
            disabled={atMax}
            title={atMax ? t("mv_max_reached") : undefined}
            className="inline-flex items-center gap-1 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800 disabled:opacity-50"
          >
            <Plus className="size-3" strokeWidth={1.75} />
            {t("mv_add_pane")}
          </button>
        </div>
      </header>

      {/* gap-px over a neutral background paints clean 1px gridlines between
          cells — avoids the "border on every side" pile-up. */}
      <div
        className={`grid min-h-0 flex-1 auto-rows-fr gap-px overflow-hidden bg-neutral-800 ${gridColsClass(
          cols,
        )}`}
      >
        {panes.map((pane) => (
          <PaneCard
            key={pane.paneId}
            sessionId={pane.sessionId}
            pureMode={pureMode}
            projects={projects}
            sessionsByProject={sessionsByProject}
            paneProject={
              pane.sessionId ? projectBySession.get(pane.sessionId) ?? null : null
            }
            canRemove={panes.length > 1}
            isFocused={focusedPane?.paneId === pane.paneId}
            dragActive={dragPaneId !== null}
            isDragSource={dragPaneId === pane.paneId}
            onFocus={() => setFocusedPaneId(pane.paneId)}
            onPick={(sid) => setPaneSession(pane.paneId, sid)}
            onNew={() => setNewPaneId(pane.paneId)}
            onRemove={() => {
              infoRef.current.delete(pane.paneId);
              removePane(pane.paneId);
            }}
            onInfo={(info) => reportInfo(pane.paneId, info)}
            onDragStartPane={() => setDragPaneId(pane.paneId)}
            onDragEndPane={() => setDragPaneId(null)}
            onDropPane={() => {
              if (dragPaneId && dragPaneId !== pane.paneId) {
                movePane(dragPaneId, pane.paneId);
              }
              setDragPaneId(null);
            }}
          />
        ))}
      </div>

      {terminalOpen && focusedSessionId && (
        <div className="h-64 shrink-0">
          <TerminalPanel
            key={focusedSessionId}
            sessionId={focusedSessionId}
            onClose={() => setTerminalOpen(false)}
          />
        </div>
      )}

      <div className="flex items-end gap-2 border-t border-neutral-800 bg-neutral-900 p-2">
        <textarea
          ref={broadcastRef}
          value={broadcast}
          onChange={(e) => setBroadcast(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              doBroadcast();
            }
          }}
          rows={1}
          placeholder={t("mv_broadcast_placeholder")}
          className="max-h-32 min-h-[34px] flex-1 resize-none overflow-y-auto rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700"
        />
        <button
          type="button"
          onClick={doBroadcast}
          title={t("mv_broadcast")}
          aria-label={t("mv_broadcast")}
          className="flex h-[34px] items-center gap-1 rounded bg-neutral-200 px-3 text-[12px] font-medium text-neutral-900 hover:bg-white"
        >
          <Send className="size-3.5" strokeWidth={1.75} />
          {t("mv_broadcast")}
        </button>
      </div>

      <NewChatModal
        open={newPaneId !== null}
        onClose={() => setNewPaneId(null)}
        onPick={onNewPanePick}
      />
    </section>
  );
}

function PaneCard({
  sessionId,
  pureMode,
  projects,
  sessionsByProject,
  paneProject,
  canRemove,
  isFocused,
  dragActive,
  isDragSource,
  onFocus,
  onPick,
  onNew,
  onRemove,
  onInfo,
  onDragStartPane,
  onDragEndPane,
  onDropPane,
}: {
  sessionId: string | null;
  pureMode: boolean;
  projects: ProjectRow[];
  sessionsByProject: SessionsByProject;
  paneProject: ProjectRow | null;
  canRemove: boolean;
  isFocused: boolean;
  dragActive: boolean;
  isDragSource: boolean;
  onFocus: () => void;
  onPick: (sessionId: string | null) => void;
  onNew: () => void;
  onRemove: () => void;
  onInfo: (info: PaneInfo) => void;
  onDragStartPane: () => void;
  onDragEndPane: () => void;
  onDropPane: () => void;
}) {
  const { t } = useTranslation("chat");
  const [kind, setKind] = useState<SessionKind | null>(null);
  const [isOver, setIsOver] = useState(false);

  // Only foreign drags (another pane) are valid drop targets here.
  const canDrop = dragActive && !isDragSource;

  // Resolve the session's kind so we know which panel to embed, and seed the
  // broadcast registry. Pure panes also report their pty id via onPtyReady.
  useEffect(() => {
    if (!sessionId) {
      setKind(null);
      return;
    }
    let cancelled = false;
    void sessionGet({ session_id: sessionId })
      .then((snap) => {
        if (cancelled || !snap) return;
        // The global pure toggle overrides a stale stored kind (see parent).
        const effective = pureMode ? "pure" : snap.kind;
        setKind(effective);
        onInfo({ sessionId, kind: effective, ptyId: null });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // onInfo is stable (useCallback in parent); sessionId + pureMode drive this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, pureMode]);

  return (
    // Capture-phase mousedown so focusing the pane works even when the click
    // lands inside the embedded panel, without swallowing the inner handlers.
    <div
      onMouseDownCapture={onFocus}
      onDragOver={(e) => {
        if (canDrop) e.preventDefault();
      }}
      onDragEnter={() => canDrop && setIsOver(true)}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
          setIsOver(false);
        }
      }}
      onDrop={(e) => {
        e.preventDefault();
        setIsOver(false);
        onDropPane();
      }}
      className={`flex min-h-0 flex-col overflow-hidden bg-neutral-950 ${
        canDrop && isOver
          ? "ring-2 ring-inset ring-emerald-500/70"
          : isFocused
            ? "ring-2 ring-inset ring-blue-500"
            : ""
      } ${isDragSource ? "opacity-40" : ""}`}
    >
      <div className="flex items-center gap-1.5 border-b border-neutral-800 bg-neutral-900 px-2 py-1">
        <span
          draggable
          onDragStart={(e) => {
            e.dataTransfer.effectAllowed = "move";
            onDragStartPane();
          }}
          onDragEnd={onDragEndPane}
          title={t("mv_drag_hint")}
          aria-label={t("mv_drag_hint")}
          className="flex shrink-0 cursor-grab items-center text-neutral-600 hover:text-neutral-300 active:cursor-grabbing"
        >
          {paneProject ? (
            <ProjectBadge
              name={paneProject.name}
              projectId={paneProject.id}
              logoPath={paneProject.logo_path}
              size={16}
            />
          ) : (
            <GripVertical className="size-4" strokeWidth={1.75} />
          )}
        </span>
        <select
          value={sessionId ?? ""}
          onChange={(e) => onPick(e.target.value || null)}
          className="min-w-0 flex-1 truncate rounded border border-neutral-800 bg-neutral-950 px-1.5 py-1 text-[11px] text-neutral-200 outline-none focus:border-neutral-700"
        >
          <option value="" className="bg-neutral-900">
            {t("mv_pick_session")}
          </option>
          {projects.map((p) => {
            const rows = sessionsByProject[p.id] ?? [];
            if (rows.length === 0) return null;
            return (
              <optgroup key={p.id} label={p.name} className="bg-neutral-900">
                {rows.map((s) => (
                  <option key={s.id} value={s.id} className="bg-neutral-900">
                    {s.title || t("mv_untitled")}
                  </option>
                ))}
              </optgroup>
            );
          })}
        </select>
        <button
          type="button"
          onClick={onNew}
          aria-label={t("mv_new_session")}
          title={t("mv_new_session")}
          className="flex size-6 items-center justify-center rounded text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
        >
          <Plus className="size-3.5" strokeWidth={1.75} />
        </button>
        {canRemove && (
          <button
            type="button"
            onClick={onRemove}
            aria-label={t("mv_remove_pane")}
            title={t("mv_remove_pane")}
            className="flex size-6 items-center justify-center rounded text-neutral-500 hover:bg-red-950/40 hover:text-red-300"
          >
            <X className="size-3.5" strokeWidth={2} />
          </button>
        )}
      </div>

      <div className="relative min-h-0 flex-1 overflow-hidden">
        {!sessionId ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 px-2 text-center text-[11px] text-neutral-600">
            {t("mv_empty_pane")}
            <button
              type="button"
              onClick={onNew}
              className="inline-flex items-center gap-1 rounded border border-neutral-700 px-2 py-1 text-neutral-300 hover:bg-neutral-800"
            >
              <Plus className="size-3" strokeWidth={1.75} />
              {t("mv_new_session")}
            </button>
          </div>
        ) : !paneProject ? (
          <div className="flex h-full items-center justify-center text-[11px] text-neutral-600">
            {t("mv_pick_session")}
          </div>
        ) : kind === "pure" ? (
          <PureClaudePanel
            key={sessionId}
            project={paneProject}
            sessionId={sessionId}
            embedded
            onPtyReady={(ptyId) => onInfo({ sessionId, kind: "pure", ptyId })}
          />
        ) : (
          <ChatPanel key={sessionId} project={paneProject} sessionId={sessionId} />
        )}
      </div>
    </div>
  );
}
