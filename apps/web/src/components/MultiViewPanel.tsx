import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Send, Sparkles, Terminal as TerminalIcon, X } from "lucide-react";
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
import {
  type MvCols,
  gridColsClass,
  maxPanes,
  useMultiViewStore,
} from "~/stores/multiViewStore.ts";
import { ChatPanel } from "~/components/ChatPanel.tsx";
import { PureClaudePanel } from "~/components/PureClaudePanel.tsx";
import { TerminalPanel } from "~/components/TerminalPanel.tsx";

interface PaneInfo {
  sessionId: string;
  kind: SessionKind | null;
  ptyId: string | null;
}

/**
 * Multi View — a grid (cols × up to 3 rows) where each pane embeds an existing
 * session (chat or pure). A broadcast bar fans the same prompt to every pane.
 * Layout persists via the multiView store.
 */
export function MultiViewPanel() {
  const { t } = useTranslation("chat");
  const projects = useProjectStore((s) => s.projects);
  const activeProjectId = useProjectStore((s) => s.activeId);
  const project = projects.find((p) => p.id === activeProjectId) ?? null;

  const panes = useMultiViewStore((s) => s.panes);
  const cols = useMultiViewStore((s) => s.cols);
  const addPane = useMultiViewStore((s) => s.addPane);
  const removePane = useMultiViewStore((s) => s.removePane);
  const setPaneSession = useMultiViewStore((s) => s.setPaneSession);
  const setCols = useMultiViewStore((s) => s.setCols);
  const setPanes = useMultiViewStore((s) => s.setPanes);

  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [broadcast, setBroadcast] = useState("");
  // The terminal dock belongs to the Multi View (not a grid pane); it follows
  // whichever pane the user last focused.
  const [focusedPaneId, setFocusedPaneId] = useState<string | null>(null);
  const [terminalOpen, setTerminalOpen] = useState(false);

  const focusedPane =
    panes.find((p) => p.paneId === focusedPaneId) ?? panes[0];
  const focusedSessionId = focusedPane?.sessionId ?? null;

  // Live registry of what each pane resolved to (kind + pty id), keyed by
  // paneId. A ref because broadcast only reads it on submit — no re-render.
  const infoRef = useRef<Map<string, PaneInfo>>(new Map());

  const refreshSessions = useCallback(async () => {
    if (!project) {
      setSessions([]);
      return;
    }
    try {
      setSessions(await sessionList({ project_id: project.id }));
    } catch {
      /* keep prior */
    }
  }, [project]);

  useEffect(() => {
    void refreshSessions();
  }, [refreshSessions, panes]);

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

  // Autofill: drop the project's sessions into the grid (running first, then
  // most recent), capped at the current column count × 3 rows.
  const autofill = useCallback(() => {
    const ordered = [...sessions].sort((a, b) => {
      const ar = a.status === "running" ? 0 : 1;
      const br = b.status === "running" ? 0 : 1;
      if (ar !== br) return ar - br;
      return b.last_activity_at.localeCompare(a.last_activity_at);
    });
    setPanes(ordered.map((s) => s.id));
  }, [sessions, setPanes]);

  if (!project) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-neutral-500">
        {t("mv_no_project")}
      </div>
    );
  }

  const atMax = panes.length >= maxPanes(cols);

  return (
    <section className="flex h-full min-h-0 flex-col bg-neutral-950">
      <header className="flex items-center justify-between gap-2 border-b border-neutral-800 bg-neutral-900 px-3 py-1.5">
        <span className="truncate text-[11px] font-medium text-neutral-200">
          {t("mv_title")} · {project.name}
        </span>
        <div className="flex items-center gap-1.5">
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
              {[3, 4, 5].map((c) => (
                <option key={c} value={c} className="bg-neutral-900">
                  {c}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            onClick={autofill}
            disabled={sessions.length === 0}
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
            sessions={sessions}
            project={project}
            canRemove={panes.length > 1}
            isFocused={focusedPane?.paneId === pane.paneId}
            onFocus={() => setFocusedPaneId(pane.paneId)}
            onPick={(sid) => setPaneSession(pane.paneId, sid)}
            onNew={async () => {
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
                setPaneSession(pane.paneId, res.session_id);
                void refreshSessions();
              } catch {
                /* ignore — surfaced nowhere critical for v1 */
              }
            }}
            onRemove={() => {
              infoRef.current.delete(pane.paneId);
              removePane(pane.paneId);
            }}
            onInfo={(info) => reportInfo(pane.paneId, info)}
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
          className="max-h-32 min-h-[34px] flex-1 resize-none rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700"
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
    </section>
  );
}

function PaneCard({
  sessionId,
  sessions,
  project,
  canRemove,
  isFocused,
  onFocus,
  onPick,
  onNew,
  onRemove,
  onInfo,
}: {
  sessionId: string | null;
  sessions: SessionSummary[];
  project: ProjectRow;
  canRemove: boolean;
  isFocused: boolean;
  onFocus: () => void;
  onPick: (sessionId: string | null) => void;
  onNew: () => void;
  onRemove: () => void;
  onInfo: (info: PaneInfo) => void;
}) {
  const { t } = useTranslation("chat");
  const [kind, setKind] = useState<SessionKind | null>(null);

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
        setKind(snap.kind);
        onInfo({ sessionId, kind: snap.kind, ptyId: null });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // onInfo is stable (useCallback in parent); sessionId drives this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  return (
    // Capture-phase mousedown so focusing the pane works even when the click
    // lands inside the embedded panel, without swallowing the inner handlers.
    <div
      onMouseDownCapture={onFocus}
      className={`flex min-h-0 flex-col overflow-hidden ${
        isFocused ? "bg-neutral-950 ring-1 ring-inset ring-blue-600/60" : "bg-neutral-950"
      }`}
    >
      <div className="flex items-center gap-1.5 border-b border-neutral-800 bg-neutral-900 px-2 py-1">
        <select
          value={sessionId ?? ""}
          onChange={(e) => onPick(e.target.value || null)}
          className="min-w-0 flex-1 truncate rounded border border-neutral-800 bg-neutral-950 px-1.5 py-1 text-[11px] text-neutral-200 outline-none focus:border-neutral-700"
        >
          <option value="" className="bg-neutral-900">
            {t("mv_pick_session")}
          </option>
          {sessions.map((s) => (
            <option key={s.id} value={s.id} className="bg-neutral-900">
              {s.title || t("mv_untitled")}
            </option>
          ))}
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
        ) : kind === "pure" ? (
          <PureClaudePanel
            key={sessionId}
            project={project}
            sessionId={sessionId}
            embedded
            onPtyReady={(ptyId) => onInfo({ sessionId, kind: "pure", ptyId })}
          />
        ) : (
          <ChatPanel key={sessionId} project={project} sessionId={sessionId} />
        )}
      </div>
    </div>
  );
}
