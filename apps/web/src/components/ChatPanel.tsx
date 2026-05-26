import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import { convertFileSrc } from "@tauri-apps/api/core";
import { parseUserMessage } from "~/lib/parseUserMessage.ts";
import { matchesKey } from "~/lib/keybindings.ts";
import { claudeLanguageDirective } from "~/lib/claudeLanguage.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import {
  playTurnCompleteChime,
  shouldNotify,
} from "~/lib/notificationSound.ts";
import { bumpBadge } from "~/lib/taskbarBadge.ts";
import { useBusyStore } from "~/stores/busyStore.ts";
import { useKeybindingsStore } from "~/stores/keybindingsStore.ts";
import { getDraft, setDraft } from "~/stores/drafts.ts";
import {
  ArrowDown,
  ArrowUp,
  Brain,
  Check,
  ChevronsUpDown,
  Clock,
  Cpu,
  GitBranch,
  ShieldAlert,
  Mic,
  MicOff,
  MessageSquarePlus,
  Paperclip,
  PauseCircle,
  PlayCircle,
  Square as SquareIcon,
  Sparkles,
  StopCircle,
  Terminal as TerminalIcon,
  X,
} from "lucide-react";
import type { ProjectRow } from "~/ipc/commands.ts";
import {
  type AssistantBlock,
  type RuntimeMode,
  type SessionSnapshot,
  type ThinkingMode,
  type ToolApprovalRequest,
  type TurnEntry,
  onSessionApproval,
  onSessionEvent,
  sessionApproveTool,
  sessionGet,
  sessionInterrupt,
  sessionRejectTool,
  sessionResume,
  sessionSendMessage,
  sessionStart,
  sessionStop,
} from "~/ipc/session.ts";
import {
  isPrimaryWorktreeId,
  type WorktreeRow,
  worktreeList,
} from "~/ipc/worktree.ts";
import { worktreeEnsureReady } from "~/ipc/indexing.ts";
import { EmptyChatState } from "~/components/EmptyChatState.tsx";
import { IndexingChip } from "~/components/IndexingChip.tsx";
import { LspChip } from "~/components/LspChip.tsx";
import { useSessionStore } from "~/stores/sessionStore.ts";
import {
  toSpeechLocale,
  useSpeechRecognition,
} from "~/hooks/useSpeechRecognition.ts";
import {
  type AttachmentInfo,
  attachmentSave,
  blobToBase64,
} from "~/ipc/attachments.ts";
import {
  type DotenvStatus,
  type EnvMode,
  type EnvStatus,
  type EnvTemplate,
  envDotenvRenderForWorktree,
  envDotenvStatusForWorktree,
  envDownForWorktree,
  envStatusForWorktree,
  envTemplateForWorktree,
  envUpForWorktree,
  sessionSetEnvMode,
} from "~/ipc/env.ts";
import { CodeBlock } from "~/components/CodeBlock.tsx";
import { ToolCallView } from "~/components/ToolCallView.tsx";
import { TurnDiffView } from "~/components/TurnDiff.tsx";

interface ChatPanelProps {
  project: ProjectRow | null;
  onToggleTerminal?: (() => void) | undefined;
  terminalOpen?: boolean;
  /**
   * When set, the panel operates on this session instead of the global active
   * one — used to embed a thread as a Multi View pane. Existing-session panes
   * never create new sessions, so the `setActive` paths stay dormant.
   */
  sessionId?: string;
}

/**
 * Leading empty string means "let Claude pick" (no `--model` flag passed).
 */
const MODELS = [
  "",
  "claude-opus-4-7",
  "claude-sonnet-4-6",
  "claude-haiku-4-5-20251001",
];

export function ChatPanel({
  project,
  onToggleTerminal,
  terminalOpen = false,
  sessionId,
}: ChatPanelProps) {
  const { t } = useTranslation("chat");
  const snapshots = useSessionStore((s) => s.snapshots);
  const storeActiveId = useSessionStore((s) => s.activeSessionId);
  // A `sessionId` prop pins the panel to one session (Multi View pane);
  // otherwise it follows the global active session.
  const activeId = sessionId ?? storeActiveId;
  const setActive = useSessionStore((s) => s.setActive);
  const hydrate = useSessionStore((s) => s.hydrate);
  const applyEvent = useSessionStore((s) => s.applyEvent);
  const setNeedsInput = useSessionStore((s) => s.setNeedsInput);
  const setBusy = useBusyStore((s) => s.setBusy);

  const [model, setModel] = useState<string>("");
  const [runtime, setRuntime] = useState<RuntimeMode>("supervised");
  const [thinking, setThinking] = useState<ThinkingMode>("auto");
  const [worktrees, setWorktrees] = useState<WorktreeRow[]>([]);
  const [worktreesLoading, setWorktreesLoading] = useState(false);
  const [worktreeId, setWorktreeId] = useState<string>("");
  /**
   * Messages typed while a turn is still streaming. They auto-flush in
   * order as soon as the busy flag flips false (TurnCompleted /
   * TurnFailed / TurnInterrupted) — same UX Claude Code has in the CLI.
   */
  const [queue, setQueue] = useState<Array<{ id: string; text: string }>>([]);
  const [envMode, setEnvMode] = useState<EnvMode>("default");
  const [envTemplate, setEnvTemplate] = useState<EnvTemplate | null>(null);
  const [envStatus, setEnvStatus] = useState<EnvStatus | null>(null);
  const [dotenvStatus, setDotenvStatus] = useState<DotenvStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Pending tool-approval prompts for the active session (supervised mode).
  // FIFO: claude asks one at a time, but we keep an array to be safe.
  const [approvals, setApprovals] = useState<ToolApprovalRequest[]>([]);

  // Worktrees for the workspace picker. Keyed on `project?.id` (string) so
  // switching projects always retriggers the fetch even if the row object
  // ref happens to be stable. The cancellation guard keeps an in-flight
  // request for an old project from clobbering the new project's data on
  // out-of-order resolution.
  const projectId = project?.id ?? null;
  useEffect(() => {
    setWorktreeId("");
    if (!projectId) {
      setWorktrees([]);
      setWorktreesLoading(false);
      return;
    }
    let cancelled = false;
    setWorktrees([]);
    setWorktreesLoading(true);
    void worktreeList({ project_id: projectId })
      .then((rows) => {
        if (!cancelled) setWorktrees(rows);
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setWorktreesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  // Imperative refresh used by children that mutate worktrees (e.g. the
  // empty-state's "create new worktree" form).
  const refreshWorktrees = useCallback(() => {
    if (!projectId) {
      setWorktrees([]);
      return;
    }
    setWorktreesLoading(true);
    void worktreeList({ project_id: projectId })
      .then(setWorktrees)
      .catch(() => {})
      .finally(() => setWorktreesLoading(false));
  }, [projectId]);

  // Detect docker template + initial status whenever the worktree picker
  // changes. We only run this when the user is about to start a session
  // (no active session) — for active sessions we use the snapshot's
  // worktree_id below.
  useEffect(() => {
    if (!worktreeId) {
      setEnvTemplate(null);
      setEnvStatus(null);
      return;
    }
    let cancelled = false;
    void envTemplateForWorktree({ worktree_id: worktreeId })
      .then((t) => {
        if (cancelled) return;
        setEnvTemplate(t);
        // Auto-pick "worktree" mode when template exists, "default" otherwise.
        setEnvMode(t.has_template ? "worktree" : "default");
      })
      .catch(() => {
        if (!cancelled) setEnvTemplate(null);
      });
    return () => {
      cancelled = true;
    };
  }, [worktreeId]);

  // Hydrate the active snapshot + auto-resume stopped/errored sessions that
  // still have a provider session id.
  useEffect(() => {
    if (!activeId) return;
    let cancelled = false;
    void sessionGet({ session_id: activeId })
      .then(async (snap) => {
        if (cancelled || !snap) return;
        hydrate(snap);
        if (
          (snap.status === "stopped" || snap.status === "errored") &&
          snap.provider_session_id
        ) {
          try {
            await sessionResume({ session_id: snap.id });
            const fresh = await sessionGet({ session_id: snap.id });
            if (!cancelled && fresh) hydrate(fresh);
          } catch (e) {
            if (!cancelled) setError(extractError(e));
          }
        }
      })
      .catch((e) => {
        if (!cancelled) setError(extractError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [activeId, hydrate]);

  // Subscribe to the active session's stream.
  useEffect(() => {
    if (!activeId) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onSessionEvent(activeId, (payload) => {
      if (cancelled) return;
      applyEvent(payload);
      // Drive the sidebar's pulsing "busy" dot for the active thread.
      const kind = payload.event.kind;
      if (kind === "TurnStarted") {
        // Working again → blue; clears any "wants input" (red) flag.
        setBusy(activeId, true);
        setNeedsInput(activeId, false);
      }
      // Turn is "yours again" on any terminal outcome — chime if the user
      // has the app in the background so they know to come back.
      const terminal =
        kind === "TurnCompleted" ||
        kind === "TurnFailed" ||
        kind === "TurnInterrupted";
      if (terminal) {
        // blue → orange: only chime if it was actually working.
        const wasBusy = useBusyStore.getState().busy[activeId];
        setBusy(activeId, false);
        // Any pending approval for this turn is moot once it ends.
        setApprovals([]);
        setNeedsInput(activeId, false);
        if (wasBusy && shouldNotify()) {
          playTurnCompleteChime();
          bumpBadge();
        }
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [activeId, applyEvent, setBusy, setNeedsInput]);

  // Subscribe to the active session's tool-approval prompts. A pending prompt
  // means the turn is paused waiting on the user. Cleared on session switch.
  useEffect(() => {
    setApprovals([]);
    if (!activeId) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onSessionApproval(activeId, (req) => {
      if (cancelled) return;
      // blue → red: Claude wants an input. Light the red bull and chime when
      // the window is backgrounded so the user knows to come decide.
      setNeedsInput(activeId, true);
      if (shouldNotify()) {
        playTurnCompleteChime();
        bumpBadge();
      }
      setApprovals((prev) =>
        prev.some((p) => p.request_id === req.request_id) ? prev : [...prev, req],
      );
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [activeId, setNeedsInput]);

  // Answering the last pending prompt clears the red bull. The turn resumes,
  // so `busy` stays true and the dot reads blue again (red → blue).
  const clearInputIfDrained = useCallback(
    (req: ToolApprovalRequest) =>
      setApprovals((prev) => {
        const next = prev.filter((p) => p.request_id !== req.request_id);
        if (next.length === 0) setNeedsInput(req.session_id, false);
        return next;
      }),
    [setNeedsInput],
  );

  const onApprove = useCallback(
    async (req: ToolApprovalRequest) => {
      clearInputIfDrained(req);
      try {
        await sessionApproveTool({
          session_id: req.session_id,
          request_id: req.request_id,
        });
      } catch (e) {
        setError(extractError(e));
      }
    },
    [clearInputIfDrained],
  );

  const onReject = useCallback(
    async (req: ToolApprovalRequest) => {
      clearInputIfDrained(req);
      try {
        await sessionRejectTool({
          session_id: req.session_id,
          request_id: req.request_id,
        });
      } catch (e) {
        setError(extractError(e));
      }
    },
    [clearInputIfDrained],
  );

  const activeSnapshot = activeId ? snapshots[activeId] : undefined;
  const isRunning = activeSnapshot?.status === "running";

  // We need to know "is there a streaming turn right now?" inside the
  // submit callback without making it depend on every snapshot tick. Keep
  // the latest value in a ref.
  const busyTurnIdRef = useRef<string | null>(null);
  useEffect(() => {
    busyTurnIdRef.current = activeSnapshot
      ? getBusyTurnId(activeSnapshot.turns)
      : null;
  }, [activeSnapshot]);

  // Drain the queue on idle. Sequential: only one in-flight at a time so
  // the order the user typed is preserved.
  useEffect(() => {
    if (!activeId) return;
    if (queue.length === 0) return;
    const busy = activeSnapshot
      ? getBusyTurnId(activeSnapshot.turns) !== null
      : false;
    if (busy) return;
    const [head, ...rest] = queue;
    if (!head) return;
    setQueue(rest);
    void sessionSendMessage({ session_id: activeId, text: head.text }).catch(
      (e) => setError(extractError(e)),
    );
  }, [activeId, activeSnapshot, queue]);

  // Drop the queue when the session is gone — leftover messages would
  // surprise the user on next start.
  useEffect(() => {
    if (!activeId) setQueue([]);
  }, [activeId]);

  // Idempotent "make this worktree ready" — covers worktrees created
  // before the eager warm-up shipped, plus every reload of an existing
  // project. Backend fast-paths when nothing needs to happen, so spamming
  // the call on every worktree change is safe. Pass `project_id` so the
  // backend can resolve the synthetic primary sentinel back to the
  // project's root.
  useEffect(() => {
    const wtId =
      activeSnapshot?.worktree_id ??
      (worktreeId
        ? worktreeId
        : worktrees.find((w) => w.is_primary)?.id ?? null);
    if (!wtId || !project) return;
    void worktreeEnsureReady({
      worktree_id: wtId,
      project_id: project.id,
    }).catch(() => {
      /* surfaced via lsp:status / indexing:progress events */
    });
  }, [activeSnapshot?.worktree_id, worktreeId, worktrees, project]);

  // For an active session that has a worktree, load template + poll status
  // so we can show 🟢/🔴 on the env chip.
  useEffect(() => {
    const wtId = activeSnapshot?.worktree_id ?? null;
    if (!wtId) {
      setEnvTemplate(null);
      setEnvStatus(null);
      setDotenvStatus(null);
      return;
    }
    let cancelled = false;
    let timer: number | undefined;
    const loadOnce = async () => {
      try {
        const tpl = await envTemplateForWorktree({ worktree_id: wtId });
        if (cancelled) return;
        setEnvTemplate(tpl);
        if (!tpl.has_template) {
          setEnvStatus(null);
          setDotenvStatus(null);
          return;
        }
        const [status, dot] = await Promise.all([
          envStatusForWorktree({ worktree_id: wtId }),
          envDotenvStatusForWorktree({ worktree_id: wtId }),
        ]);
        if (!cancelled) {
          setEnvStatus(status);
          setDotenvStatus(dot);
        }
      } catch {
        /* leave previous values in place */
      }
    };
    void loadOnce();
    timer = window.setInterval(loadOnce, 5000);
    return () => {
      cancelled = true;
      if (timer) window.clearInterval(timer);
    };
  }, [activeSnapshot?.worktree_id]);

  const onRegenerateDotenv = useCallback(async () => {
    const wtId = activeSnapshot?.worktree_id ?? null;
    if (!wtId) return;
    try {
      const result = await envDotenvRenderForWorktree({ worktree_id: wtId });
      if (result.kind === "manual_override") {
        setError(t("env_dotenv_manual_override", { path: result.path }));
      } else if (result.kind === "no_template") {
        setError(t("env_dotenv_no_template"));
      } else {
        // Refresh status so the stale flag clears immediately.
        const fresh = await envDotenvStatusForWorktree({ worktree_id: wtId });
        setDotenvStatus(fresh);
      }
    } catch (e) {
      setError(extractError(e));
    }
  }, [activeSnapshot?.worktree_id, t]);

  // Sync envMode state with session snapshot for already-running sessions.
  useEffect(() => {
    if (activeSnapshot) {
      setEnvMode(activeSnapshot.env_mode);
    }
  }, [activeSnapshot?.id, activeSnapshot?.env_mode]);
  const canResume =
    !!activeSnapshot &&
    activeSnapshot.status !== "running" &&
    !!activeSnapshot.provider_session_id;

  const onResume = useCallback(async () => {
    if (!activeSnapshot) return;
    try {
      await sessionResume({ session_id: activeSnapshot.id });
      const fresh = await sessionGet({ session_id: activeSnapshot.id });
      if (fresh) hydrate(fresh);
    } catch (e) {
      setError(extractError(e));
    }
  }, [activeSnapshot, hydrate]);

  const onStop = useCallback(async () => {
    if (!activeId) return;
    try {
      await sessionStop({ session_id: activeId });
    } catch (e) {
      setError(extractError(e));
    }
  }, [activeId]);

  const onSendOrStart = useCallback(
    async (text: string) => {
      if (!project) return;
      setError(null);
      // Streaming a turn? Park the message — the drain effect picks it up
      // when the in-flight turn finishes.
      if (activeId && busyTurnIdRef.current) {
        setQueue((q) => [
          ...q,
          {
            id: crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`,
            text,
          },
        ]);
        return;
      }
      // If a session is already running but idle, just send.
      if (activeId && isRunning) {
        await sessionSendMessage({ session_id: activeId, text });
        return;
      }
      // No live session — start one with the picked options, then send the
      // first message immediately so the user gets a one-click thread.
      const wt = worktreeId
        ? worktrees.find((w) => w.id === worktreeId) ?? null
        : null;
      // The synthetic primary maps to "no worktree" on the backend (uses the
      // project root). Real user-created worktrees pass their id through.
      const wtIdToSend =
        wt && !isPrimaryWorktreeId(wt.id) ? wt.id : undefined;
      const res = await sessionStart({
        project_id: project.id,
        provider_id: "claude",
        environment: project.environment,
        cwd: wt ? wt.path : project.root_path,
        model,
        thinking,
        runtime,
        env_mode: envMode,
        system_prompt: claudeLanguageDirective(
          useAppSettingsStore.getState().claudeLanguage,
        ),
        ...(wtIdToSend ? { worktree_id: wtIdToSend } : {}),
      });
      setActive(res.session_id);
      // Auto-up the worktree env if the session opted into it and the stack
      // isn't already running. Fire-and-forget — Claude can start receiving
      // the user message in parallel; the docker compose runs in a new tab.
      if (
        wt &&
        envMode === "worktree" &&
        envTemplate?.has_template &&
        !envStatus?.up
      ) {
        void envUpForWorktree({
          worktree_id: wt.id,
          session_id: res.session_id,
        }).catch(() => {});
      }
      await sessionSendMessage({ session_id: res.session_id, text });
    },
    [
      project,
      activeId,
      isRunning,
      model,
      thinking,
      runtime,
      envMode,
      worktreeId,
      worktrees,
      setActive,
    ],
  );

  const onChangeEnvMode = useCallback(
    async (mode: EnvMode) => {
      setEnvMode(mode);
      if (activeId) {
        try {
          await sessionSetEnvMode({ session_id: activeId, mode });
        } catch (e) {
          setError(extractError(e));
        }
      }
    },
    [activeId],
  );

  const onEnvUp = useCallback(async () => {
    if (!activeSnapshot?.worktree_id || !activeId) return;
    try {
      await envUpForWorktree({
        worktree_id: activeSnapshot.worktree_id,
        session_id: activeId,
      });
    } catch (e) {
      setError(extractError(e));
    }
  }, [activeId, activeSnapshot?.worktree_id]);

  const onEnvDown = useCallback(async () => {
    if (!activeSnapshot?.worktree_id || !activeId) return;
    if (!window.confirm(t("env_down_confirm"))) return;
    try {
      await envDownForWorktree({
        worktree_id: activeSnapshot.worktree_id,
        session_id: activeId,
      });
    } catch (e) {
      setError(extractError(e));
    }
  }, [activeId, activeSnapshot?.worktree_id, t]);

  if (!project) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-neutral-500">
        {t("no_project")}
      </div>
    );
  }

  const busyTurnId = activeSnapshot
    ? getBusyTurnId(activeSnapshot.turns)
    : null;

  return (
    <section className="flex h-full min-h-0 flex-col bg-neutral-950">
      <header className="flex items-center gap-3 border-b border-neutral-800 px-4 py-1.5 text-[11px] text-neutral-500">
        <div className="flex items-center gap-2">
          {activeSnapshot ? (
            <>
              <StatusBadge status={activeSnapshot.status} />
              {activeSnapshot.title && (
                <span className="truncate text-neutral-300">
                  {activeSnapshot.title}
                </span>
              )}
              <span className="truncate text-neutral-600">
                · {activeSnapshot.model || t("model_auto_short")}
              </span>
              {(() => {
                // `null` worktree_id means the session runs at the project
                // root — render that as the primary card's branch.
                const wt =
                  activeSnapshot.worktree_id == null
                    ? worktrees.find((w) => w.is_primary)
                    : worktrees.find(
                        (w) => w.id === activeSnapshot.worktree_id,
                      );
                return wt ? (
                  <span className="inline-flex items-center gap-1 truncate text-neutral-500">
                    · <GitBranch className="size-3" strokeWidth={1.75} />
                    {wt.branch || "main"}
                  </span>
                ) : null;
              })()}
            </>
          ) : (
            <span>{t("no_session")}</span>
          )}
        </div>
        <div className="flex flex-1 items-center justify-end gap-2">
          {(() => {
            const wtId =
              activeSnapshot?.worktree_id ??
              (worktreeId
                ? worktreeId
                : worktrees.find((w) => w.is_primary)?.id ?? null);
            return (
              <>
                <IndexingChip worktreeId={wtId} />
                <LspChip worktreeId={wtId} />
              </>
            );
          })()}
        </div>
        <div className="flex items-center gap-1">
          {onToggleTerminal && activeSnapshot && (
            <button
              type="button"
              onClick={onToggleTerminal}
              aria-label={t("terminal_heading")}
              title={t("terminal_heading")}
              className={`flex size-6 items-center justify-center rounded transition ${
                terminalOpen
                  ? "bg-neutral-800 text-neutral-100"
                  : "text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
              }`}
            >
              <TerminalIcon className="size-3.5" strokeWidth={1.75} />
            </button>
          )}
          {canResume && (
            <button
              type="button"
              onClick={() => void onResume()}
              className="inline-flex items-center gap-1 rounded border border-emerald-900/60 px-2 py-0.5 text-emerald-300 hover:bg-emerald-950/40"
            >
              <PlayCircle className="size-3" strokeWidth={1.75} />
              {t("resume_session")}
            </button>
          )}
          {isRunning && (
            <button
              type="button"
              onClick={() => void onStop()}
              className="inline-flex items-center gap-1 rounded border border-neutral-800 px-2 py-0.5 text-neutral-300 hover:bg-neutral-800"
            >
              <PauseCircle className="size-3" strokeWidth={1.75} />
              {t("stop")}
            </button>
          )}
        </div>
      </header>

      {error && (
        <div className="border-b border-red-900/50 bg-red-950/30 px-4 py-1.5 text-[11px] text-red-200">
          {t("error", { message: error })}
          <button
            type="button"
            onClick={() => setError(null)}
            className="ml-2 text-red-300 hover:text-red-100"
          >
            ×
          </button>
        </div>
      )}

      {activeSnapshot ? (
        <Thread snapshot={activeSnapshot} />
      ) : (
        <EmptyChatState
          projectId={project.id}
          projectName={project.name}
          worktrees={worktrees}
          loading={worktreesLoading}
          selectedWorktreeId={worktreeId || null}
          onSelectWorktree={(id) => setWorktreeId(id ?? "")}
          onWorktreesChanged={refreshWorktrees}
        />
      )}

      {approvals.length > 0 && (
        <ApprovalBar
          requests={approvals}
          onApprove={(r) => void onApprove(r)}
          onReject={(r) => void onReject(r)}
        />
      )}

      <Composer
        onSend={onSendOrStart}
        onInterrupt={
          activeId && busyTurnId
            ? async () => {
                try {
                  await sessionInterrupt({
                    session_id: activeId,
                    turn_id: busyTurnId,
                  });
                } catch (e) {
                  setError(extractError(e));
                }
              }
            : null
        }
        sessionKey={activeId ?? "new"}
        busy={!!busyTurnId}
        startsNewSession={!isRunning}
        queue={queue}
        onRemoveFromQueue={(id) =>
          setQueue((q) => q.filter((m) => m.id !== id))
        }
        bottomBar={
          <BottomBar
            model={model}
            onModel={setModel}
            runtime={runtime}
            onRuntime={setRuntime}
            thinking={thinking}
            onThinking={setThinking}
            disabled={isRunning}
            envMode={envMode}
            onEnvMode={(m) => void onChangeEnvMode(m)}
            envTemplate={envTemplate}
            envStatus={envStatus}
            dotenvStatus={dotenvStatus}
            onEnvUp={() => void onEnvUp()}
            onEnvDown={() => void onEnvDown()}
            onRegenerateDotenv={() => void onRegenerateDotenv()}
          />
        }
      />
    </section>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// Thread

function Thread({
  snapshot,
}: {
  snapshot: SessionSnapshot | undefined;
}) {
  const { t } = useTranslation("chat");
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [nearBottom, setNearBottom] = useState(true);
  const [pending, setPending] = useState(0);

  // Re-evaluate proximity whenever the user scrolls.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const fromBottom =
        el.scrollHeight - (el.scrollTop + el.clientHeight);
      const isNear = fromBottom < 80;
      setNearBottom(isNear);
      if (isNear) setPending(0);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  const turnCount = snapshot?.turns.length ?? 0;
  const blockCount = snapshot?.turns.at(-1)?.blocks.length ?? 0;
  const sessionId = snapshot?.id ?? null;

  // Reset pending counter on session switch; initial mount anchors to bottom.
  useEffect(() => {
    setPending(0);
    setNearBottom(true);
    requestAnimationFrame(() => {
      const el = scrollRef.current;
      if (el) el.scrollTop = el.scrollHeight;
    });
  }, [sessionId]);

  // Sticky auto-scroll — only when user is already anchored at the bottom.
  // Otherwise bump the "N new" pill.
  useEffect(() => {
    if (!sessionId) return;
    const el = scrollRef.current;
    if (!el) return;
    if (nearBottom) {
      el.scrollTop = el.scrollHeight;
    } else {
      setPending((c) => c + 1);
    }
    // We intentionally depend on the counters, not `nearBottom`, so toggling
    // proximity alone doesn't trigger a jump.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [turnCount, blockCount, sessionId]);

  const scrollToBottom = () => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
    setPending(0);
  };

  if (!snapshot) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 px-4 py-8 text-center text-neutral-500">
        <Sparkles className="size-8 text-neutral-700" strokeWidth={1.5} />
        <p className="text-sm">{t("no_session")}</p>
        <p className="text-[11px] text-neutral-600">{t("send_to_start_hint")}</p>
      </div>
    );
  }
  if (snapshot.turns.length === 0) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 px-4 py-8 text-center text-neutral-500">
        <MessageSquarePlus className="size-8 text-neutral-700" strokeWidth={1.5} />
        <p className="text-sm">{t("empty_thread")}</p>
      </div>
    );
  }
  return (
    <div className="relative min-h-0 flex-1">
      <div ref={scrollRef} className="h-full overflow-y-auto px-4 py-4">
        <div className="mx-auto flex max-w-3xl flex-col gap-5">
          {snapshot.turns.map((turn) => (
            <TurnView key={turn.id} turn={turn} sessionId={snapshot.id} />
          ))}
        </div>
      </div>
      {!nearBottom && (
        <button
          type="button"
          onClick={scrollToBottom}
          className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-neutral-700 bg-neutral-900/95 px-3 py-1 text-[11px] text-neutral-200 shadow-lg shadow-black/40 backdrop-blur hover:bg-neutral-800"
        >
          <ArrowDown className="size-3" strokeWidth={1.75} />
          {pending > 0
            ? t("scroll_new_messages", { count: pending })
            : t("scroll_to_bottom")}
        </button>
      )}
    </div>
  );
}

// Pull a human one-liner out of a tool's input (command / path / url), falling
// back to compact JSON so the user can see what they're approving.
function approvalDetail(input: unknown): string {
  if (input && typeof input === "object") {
    const o = input as Record<string, unknown>;
    const v = o.command ?? o.file_path ?? o.path ?? o.url ?? o.pattern;
    if (typeof v === "string" && v.trim()) return v;
  }
  try {
    return JSON.stringify(input);
  } catch {
    return "";
  }
}

function ApprovalBar({
  requests,
  onApprove,
  onReject,
}: {
  requests: ToolApprovalRequest[];
  onApprove: (req: ToolApprovalRequest) => void;
  onReject: (req: ToolApprovalRequest) => void;
}) {
  const { t } = useTranslation("chat");
  return (
    <div className="border-t border-amber-900/40 bg-amber-950/20 px-4 py-2">
      <div className="mx-auto flex max-w-3xl flex-col gap-2">
        {requests.map((req) => (
          <div
            key={req.request_id}
            className="flex items-start gap-3 rounded-lg border border-amber-800/50 bg-amber-950/30 px-3 py-2"
          >
            <ShieldAlert
              className="mt-0.5 size-4 shrink-0 text-amber-300"
              strokeWidth={1.75}
            />
            <div className="min-w-0 flex-1">
              <div className="text-[11px] font-medium text-amber-100">
                {t("approval_heading", { tool: req.tool_name })}
              </div>
              <pre className="mt-0.5 max-h-24 overflow-auto whitespace-pre-wrap break-all font-mono text-[10px] text-amber-200/80">
                {approvalDetail(req.input)}
              </pre>
            </div>
            <div className="flex shrink-0 gap-1.5">
              <button
                type="button"
                onClick={() => onReject(req)}
                className="flex items-center gap-1 rounded border border-neutral-700 px-2 py-1 text-[11px] text-neutral-300 hover:bg-neutral-800"
              >
                <X className="size-3" strokeWidth={2} />
                {t("approval_deny")}
              </button>
              <button
                type="button"
                onClick={() => onApprove(req)}
                className="flex items-center gap-1 rounded bg-amber-400 px-2 py-1 text-[11px] font-medium text-amber-950 hover:bg-amber-300"
              >
                <Check className="size-3" strokeWidth={2.5} />
                {t("approval_allow")}
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function TurnView({ turn, sessionId }: { turn: TurnEntry; sessionId: string }) {
  const { t } = useTranslation("chat");
  const parsed = useMemo(() => parseUserMessage(turn.user_text), [turn.user_text]);
  return (
    <div className="flex flex-col gap-2">
      <div className="ml-auto max-w-[85%] rounded-2xl bg-neutral-900 px-3.5 py-2 text-sm text-neutral-100">
        <div className="mb-0.5 text-[9px] uppercase tracking-wide text-neutral-500">
          {t("turn_user")}
        </div>
        {parsed.images.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-2">
            {parsed.images.map((path) => (
              <a
                key={path}
                href={convertFileSrc(path)}
                target="_blank"
                rel="noreferrer"
                className="block overflow-hidden rounded-md border border-neutral-800 bg-neutral-950"
              >
                <img
                  src={convertFileSrc(path)}
                  alt=""
                  className="block max-h-56 max-w-full object-contain"
                />
              </a>
            ))}
          </div>
        )}
        {parsed.text && (
          <div className="whitespace-pre-wrap">{parsed.text}</div>
        )}
      </div>
      <TurnBody turnId={turn.id} blocks={turn.blocks} />
      {turn.status === "streaming" && (
        <div className="flex items-center gap-2 text-[11px] text-neutral-500">
          <span className="inline-block size-1.5 animate-pulse rounded-full bg-emerald-400" />
          {t("turn_streaming_label")}
        </div>
      )}
      {turn.status === "interrupted" && (
        <div className="inline-flex items-center gap-1.5 self-start rounded border border-amber-900/50 bg-amber-950/20 px-2 py-0.5 text-[11px] text-amber-200">
          <StopCircle className="size-3" strokeWidth={1.75} />
          {t("turn_interrupted_by_user")}
        </div>
      )}
      {turn.status === "failed" && turn.error_message && (
        <div className="rounded-md border border-red-900/60 bg-red-950/30 px-3 py-2 text-xs text-red-200">
          {t("turn_failed", { message: turn.error_message })}
        </div>
      )}
      {turn.status === "completed" && turn.input_tokens !== null && (
        <div className="text-[10px] text-neutral-500">
          {t("usage", {
            in: turn.input_tokens ?? 0,
            out: turn.output_tokens ?? 0,
            cost:
              turn.total_cost_usd != null
                ? turn.total_cost_usd.toFixed(4)
                : "0",
          })}
        </div>
      )}
      {(turn.status === "completed" || turn.status === "failed") && (
        <TurnDiffView sessionId={sessionId} turnId={turn.id} />
      )}
    </div>
  );
}

/**
 * Walks the assistant blocks and pairs every `tool_use` with its matching
 * `tool_result` (by `tool_use_id`) so each tool call renders as one unit via
 * `ToolCallView`. Text/thinking blocks fall through unchanged.
 */
function TurnBody({
  turnId,
  blocks,
}: {
  turnId: string;
  blocks: AssistantBlock[];
}) {
  // Index every tool_result so we can attach it to its tool_use when we
  // encounter that use in order.
  const resultByUseId = new Map<
    string,
    Extract<AssistantBlock, { kind: "tool_result" }>
  >();
  for (const block of blocks) {
    if (block.kind === "tool_result") {
      resultByUseId.set(block.tool_use_id, block);
    }
  }
  return (
    <>
      {blocks.map((block, i) => {
        if (block.kind === "tool_use") {
          return (
            <ToolCallView
              key={`${turnId}:${i}`}
              use={block}
              result={resultByUseId.get(block.id)}
            />
          );
        }
        if (block.kind === "tool_result") {
          // Already rendered inside the paired tool_use above.
          return null;
        }
        return <BlockView key={`${turnId}:${i}`} block={block} />;
      })}
    </>
  );
}

function BlockView({ block }: { block: AssistantBlock }) {
  const { t } = useTranslation("chat");
  switch (block.kind) {
    case "text":
      return <AssistantTextBlock text={block.text || ""} />;
    case "thinking":
      // Claude sometimes emits empty thinking blocks (signature-only
      // thinking, or aborted reasoning). Hiding them keeps the chat clean.
      if (!block.text || !block.text.trim()) return null;
      return (
        <details className="rounded-lg border border-neutral-800 bg-neutral-950 px-3 py-2 text-xs text-neutral-400">
          <summary className="flex cursor-pointer items-center gap-1.5 text-[10px] uppercase tracking-wide text-neutral-500">
            <Brain className="size-3" strokeWidth={1.75} />
            {t("turn_thinking")}
          </summary>
          <pre className="mt-2 whitespace-pre-wrap text-xs">{block.text}</pre>
        </details>
      );
    case "tool_use":
      // Rendered via ToolCallView in TurnBody; leave a placeholder so the
      // switch is exhaustive and tsc stays happy.
      return (
        <details className="rounded-lg border border-amber-900/40 bg-amber-950/10 px-3 py-2 text-xs text-amber-200">
          <summary className="cursor-pointer text-[10px] uppercase tracking-wide text-amber-400/70">
            {t("turn_tool_use", { name: block.name })}
          </summary>
          <pre className="mt-2 whitespace-pre-wrap font-mono text-[11px] text-amber-100">
            {JSON.stringify(block.input, null, 2)}
          </pre>
        </details>
      );
    case "tool_result":
      return (
        <details
          className={`rounded-lg px-3 py-2 text-xs ${
            block.is_error
              ? "border border-red-900/40 bg-red-950/20 text-red-200"
              : "border border-neutral-800 bg-neutral-950 text-neutral-300"
          }`}
        >
          <summary className="cursor-pointer text-[10px] uppercase tracking-wide opacity-70">
            {block.is_error ? t("turn_tool_result_error") : t("turn_tool_result")}
          </summary>
          <pre className="mt-2 whitespace-pre-wrap font-mono text-[11px]">
            {typeof block.output === "string"
              ? block.output
              : JSON.stringify(block.output, null, 2)}
          </pre>
        </details>
      );
  }
}

function AssistantTextBlock({ text }: { text: string }) {
  const { t } = useTranslation("chat");
  const [copied, setCopied] = useState(false);

  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard blocked — silently ignore */
    }
  };

  return (
    <div className="group relative rounded-2xl bg-neutral-900/60 px-4 py-3">
      <button
        type="button"
        onClick={() => void onCopy()}
        className="absolute right-2 top-2 rounded border border-neutral-800 px-1.5 py-0 text-[10px] text-neutral-400 opacity-0 transition hover:bg-neutral-800 group-hover:opacity-100"
      >
        {copied ? t("copied") : t("copy_block")}
      </button>
      <div className="prose prose-invert prose-sm max-w-none text-sm text-neutral-100">
        <ReactMarkdown
          components={{
            code: ({ className, children, ...props }) => {
              const raw = String(children ?? "").replace(/\n$/, "");
              const isFenced =
                typeof className === "string" && className.startsWith("language-");
              if (!isFenced) {
                return <CodeBlock code={raw} inline {...props} />;
              }
              const lang = className.replace(/^language-/, "");
              return <CodeBlock code={raw} lang={lang} />;
            },
            pre: ({ children }) => <>{children}</>,
          }}
        >
          {text}
        </ReactMarkdown>
      </div>
    </div>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// Composer

interface ComposerProps {
  onSend: (text: string) => Promise<void>;
  onInterrupt: (() => Promise<void>) | null;
  sessionKey: string;
  busy: boolean;
  startsNewSession: boolean;
  queue: Array<{ id: string; text: string }>;
  onRemoveFromQueue: (id: string) => void;
  bottomBar: React.ReactNode;
}

interface ComposerAttachment {
  info: AttachmentInfo;
  previewUrl: string;
}

function Composer({
  onSend,
  onInterrupt,
  sessionKey,
  busy,
  startsNewSession,
  queue,
  onRemoveFromQueue,
  bottomBar,
}: ComposerProps) {
  const { t, i18n } = useTranslation("chat");
  // Seed from the per-session draft store so a remount (triggered by switching
  // conversations) restores what the user had typed instead of clearing it.
  const [text, setText] = useState(() => getDraft(sessionKey));
  const [sending, setSending] = useState(false);
  const [attachments, setAttachments] = useState<ComposerAttachment[]>([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const speech = useSpeechRecognition({
    lang: toSpeechLocale(i18n.resolvedLanguage ?? i18n.language),
    onFinal: (chunk) => {
      setText((prev) => {
        const trimmedPrev = prev.replace(/\s+$/u, "");
        const glue = trimmedPrev.length > 0 ? " " : "";
        return `${trimmedPrev}${glue}${chunk.trim()}`;
      });
    },
  });

  // Ctrl/Cmd+click on the mic dictates and then auto-submits once recognition
  // ends. Armed via a ref so the value survives until `onend`; mirrored to
  // state only for the button's affordance.
  const autoSubmitOnEndRef = useRef(false);
  const [autoSubmitArmed, setAutoSubmitArmed] = useState(false);
  const prevListeningRef = useRef(false);

  const onMicClick = (e: React.MouseEvent) => {
    if (speech.listening) {
      // Second click ends the session; the listening effect handles the
      // auto-submit when it was armed.
      speech.stop();
      return;
    }
    const auto = e.ctrlKey || e.metaKey;
    autoSubmitOnEndRef.current = auto;
    setAutoSubmitArmed(auto);
    speech.start();
  };

  useEffect(() => {
    textareaRef.current?.focus();
  }, [sessionKey]);

  // Auto-grow the textarea with the content, capped so it can't eat the thread.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    const capped = Math.min(el.scrollHeight, 240);
    el.style.height = `${capped}px`;
  }, [text]);

  // Mirror the draft into the per-session store on every change so it survives
  // the remount that happens when the user switches conversations and back.
  useEffect(() => {
    setDraft(sessionKey, text);
  }, [sessionKey, text]);

  // Release preview blob URLs when the composer unmounts or the set of
  // attachments shrinks.
  useEffect(() => {
    return () => {
      attachments.forEach((a) => URL.revokeObjectURL(a.previewUrl));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const addFromBlobs = async (blobs: Blob[]) => {
    const accepted = blobs.filter((b) => /^image\/(png|jpe?g|webp|gif)$/i.test(b.type));
    if (accepted.length === 0) return;
    setAttachError(null);
    for (const blob of accepted) {
      try {
        const base64 = await blobToBase64(blob);
        const info = await attachmentSave({
          bucket_id: sessionKey,
          mime: blob.type,
          data_base64: base64,
          filename: (blob as File).name ?? undefined,
        });
        const previewUrl = URL.createObjectURL(blob);
        setAttachments((prev) => [...prev, { info, previewUrl }]);
      } catch (e) {
        setAttachError(extractError(e));
      }
    }
  };

  const removeAttachment = (path: string) => {
    setAttachments((prev) => {
      const target = prev.find((a) => a.info.path === path);
      if (target) URL.revokeObjectURL(target.previewUrl);
      return prev.filter((a) => a.info.path !== path);
    });
  };

  const submit = async () => {
    const trimmed = text.trim();
    // Sending mid-stream is OK now — the upstream handler routes it into
    // the queue. We only block on local `sending` to stop double-submits.
    if ((!trimmed && attachments.length === 0) || sending) return;
    setSending(true);
    const prefix = attachments.map((a) => `@${a.info.path}`).join(" ");
    const payload = prefix
      ? `${prefix}${trimmed ? `\n\n${trimmed}` : ""}`
      : trimmed;
    const sentAttachments = attachments;
    // Clear optimistically so the textarea empties immediately instead of
    // staying full while we await the streamed response.
    setText("");
    setAttachments([]);
    try {
      await onSend(payload);
      sentAttachments.forEach((a) => URL.revokeObjectURL(a.previewUrl));
    } catch (e) {
      // Surfaces via the panel-level error handler in onSend already.
      // Restore the draft so the user doesn't lose their input.
      console.error(e);
      setText(trimmed);
      setAttachments(sentAttachments);
    } finally {
      setSending(false);
    }
  };

  // When a Ctrl+click-armed dictation ends, fire the submit. Runs on the
  // listening true→false edge so the final transcript chunk (applied in the
  // preceding render) is already in `text`.
  useEffect(() => {
    const wasListening = prevListeningRef.current;
    prevListeningRef.current = speech.listening;
    if (wasListening && !speech.listening && autoSubmitOnEndRef.current) {
      autoSubmitOnEndRef.current = false;
      setAutoSubmitArmed(false);
      void submit();
    }
  }, [speech.listening, submit]);

  const onPaste = async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const items = e.clipboardData.items;
    if (!items) return;
    const blobs: Blob[] = [];
    for (let i = 0; i < items.length; i += 1) {
      const item = items[i]!;
      if (item.kind === "file") {
        const file = item.getAsFile();
        if (file) blobs.push(file);
      }
    }
    if (blobs.length > 0) {
      e.preventDefault();
      await addFromBlobs(blobs);
    }
  };

  const onPickFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files || files.length === 0) return;
    const blobs: Blob[] = [];
    for (let i = 0; i < files.length; i += 1) blobs.push(files[i]!);
    e.target.value = ""; // allow picking the same file twice in a row
    await addFromBlobs(blobs);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void submit();
      return;
    }
    if (e.key === "Escape" && onInterrupt) {
      e.preventDefault();
      void onInterrupt();
    }
  };

  // Global interrupt shortcut — pulled from user keybindings (default Esc).
  const interruptBinding = useKeybindingsStore((s) => s.bindings.interrupt);
  useEffect(() => {
    if (!onInterrupt) return;
    const onKey = (e: KeyboardEvent) => {
      if (matchesKey(e, interruptBinding)) void onInterrupt();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onInterrupt, interruptBinding]);

  const placeholder = useMemo(
    () =>
      busy
        ? t("composer_placeholder_busy")
        : startsNewSession
          ? t("composer_placeholder_new")
          : t("composer_placeholder"),
    [busy, startsNewSession, t],
  );

  return (
    <footer className="shrink-0 border-t border-neutral-800 bg-neutral-950 px-4 py-3">
      <div className="mx-auto max-w-3xl">
        {queue.length > 0 && (
          <div className="mb-2 flex flex-col gap-1 rounded-lg border border-neutral-800 bg-neutral-900/40 px-2.5 py-1.5">
            <div className="flex items-center gap-1.5 text-[9px] uppercase tracking-wider text-neutral-500">
              <Clock className="size-3" strokeWidth={1.75} />
              {t("composer_queued_heading", { count: queue.length })}
            </div>
            <ul className="flex flex-col gap-0.5">
              {queue.map((m) => (
                <li
                  key={m.id}
                  className="flex items-center gap-1.5 rounded bg-neutral-900/70 px-2 py-1 text-[11px] text-neutral-300"
                >
                  <span className="min-w-0 flex-1 truncate">
                    {m.text || "(empty)"}
                  </span>
                  <button
                    type="button"
                    onClick={() => onRemoveFromQueue(m.id)}
                    aria-label={t("composer_queued_remove")}
                    title={t("composer_queued_remove")}
                    className="flex size-4 items-center justify-center rounded text-neutral-500 hover:bg-red-950/40 hover:text-red-300"
                  >
                    <X className="size-2.5" strokeWidth={2} />
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
        <div className="flex flex-col rounded-2xl border border-neutral-800 bg-neutral-900 focus-within:border-neutral-700">
          {attachments.length > 0 && (
            <div className="flex flex-wrap gap-2 border-b border-neutral-800 px-3 pb-2 pt-3">
              {attachments.map((a) => (
                <div
                  key={a.info.path}
                  className="group relative overflow-hidden rounded-md border border-neutral-800 bg-neutral-950"
                >
                  <img
                    src={a.previewUrl}
                    alt={a.info.filename}
                    className="size-14 object-cover"
                  />
                  <button
                    type="button"
                    onClick={() => removeAttachment(a.info.path)}
                    aria-label={t("attachment_remove")}
                    title={t("attachment_remove")}
                    className="absolute right-0.5 top-0.5 flex size-4 items-center justify-center rounded bg-black/70 text-neutral-200 opacity-0 transition hover:bg-red-900/80 group-hover:opacity-100"
                  >
                    <X className="size-2.5" strokeWidth={2} />
                  </button>
                </div>
              ))}
            </div>
          )}
          {attachError && (
            <p className="mx-3 mt-2 rounded border border-red-900/50 bg-red-950/30 px-2.5 py-1.5 text-[10px] text-red-200">
              {t("attachment_error", { message: attachError })}
            </p>
          )}
          <textarea
            ref={textareaRef}
            rows={2}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
            onPaste={(e) => void onPaste(e)}
            placeholder={placeholder}
            className="min-h-[52px] w-full resize-none overflow-y-auto rounded-2xl border-0 bg-transparent px-4 py-3 text-sm text-neutral-100 shadow-none outline-none ring-0 placeholder:text-neutral-500 focus:border-0 focus:outline-none focus:ring-0"
          />
          {speech.listening && (
            <div className="mx-3 mb-2 flex items-center gap-2 text-[11px] text-emerald-300">
              <span className="relative flex size-2">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-60" />
                <span className="relative inline-flex size-2 rounded-full bg-emerald-400" />
              </span>
              <span className="truncate text-neutral-400">
                {speech.interim ||
                  (autoSubmitArmed
                    ? t("speech_listening_autosubmit")
                    : t("speech_listening"))}
              </span>
            </div>
          )}
          {speech.error && !speech.listening && (
            <p className="mx-3 mb-2 rounded border border-amber-900/50 bg-amber-950/20 px-2.5 py-1.5 text-[10px] text-amber-200">
              {t("speech_error", { message: speech.error })}
            </p>
          )}
          {text.trim().startsWith("/") && (
            <p className="mx-3 mb-2 rounded border border-amber-900/50 bg-amber-950/20 px-2.5 py-1.5 text-[10px] text-amber-200">
              {t("slash_hint")}
            </p>
          )}
          <div className="flex items-center justify-between gap-2 px-2 py-1.5">
            <div className="flex flex-wrap items-center gap-1 text-[11px]">
              {bottomBar}
            </div>
            <div className="flex items-center gap-1">
              <input
                ref={fileInputRef}
                type="file"
                accept="image/png,image/jpeg,image/webp,image/gif"
                multiple
                onChange={(e) => void onPickFile(e)}
                className="hidden"
              />
              <button
                type="button"
                onClick={() => fileInputRef.current?.click()}
                aria-label={t("attachment_pick")}
                title={t("attachment_pick")}
                className="flex size-7 items-center justify-center rounded-md border border-neutral-800 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
              >
                <Paperclip className="size-3.5" strokeWidth={1.75} />
              </button>
              {speech.supported && (
                <button
                  type="button"
                  onClick={onMicClick}
                  aria-label={
                    speech.listening
                      ? autoSubmitArmed
                        ? t("speech_stop_autosubmit")
                        : t("speech_stop")
                      : t("speech_start")
                  }
                  title={
                    speech.listening
                      ? autoSubmitArmed
                        ? t("speech_listening_autosubmit")
                        : t("speech_stop")
                      : `${t("speech_start")} · ${t("speech_autosubmit_hint")}`
                  }
                  className={`flex size-7 items-center justify-center rounded-md border transition ${
                    speech.listening
                      ? autoSubmitArmed
                        ? "border-sky-700/60 bg-sky-950/40 text-sky-300"
                        : "border-emerald-800/60 bg-emerald-950/40 text-emerald-300"
                      : "border-neutral-800 text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
                  }`}
                >
                  {speech.listening ? (
                    <MicOff className="size-3.5" strokeWidth={1.75} />
                  ) : (
                    <Mic className="size-3.5" strokeWidth={1.75} />
                  )}
                </button>
              )}
              {onInterrupt && (
                <button
                  type="button"
                  onClick={() => void onInterrupt()}
                  aria-label={t("interrupt")}
                  title={t("interrupt")}
                  className="flex size-7 items-center justify-center rounded-md border border-amber-900/60 text-amber-300 hover:bg-amber-950/40"
                >
                  <StopCircle className="size-3.5" strokeWidth={1.75} />
                </button>
              )}
              <button
                type="button"
                disabled={
                  sending ||
                  (text.trim() === "" && attachments.length === 0)
                }
                onClick={() => void submit()}
                aria-label={busy ? t("queue") : t("send")}
                title={
                  busy
                    ? t("queue_hint")
                    : startsNewSession
                      ? t("start_session")
                      : t("send")
                }
                className={`flex size-7 items-center justify-center rounded-md transition disabled:cursor-not-allowed disabled:bg-neutral-800 disabled:text-neutral-600 ${
                  busy
                    ? "bg-amber-200 text-amber-950 hover:bg-amber-100"
                    : "bg-neutral-200 text-neutral-900 hover:bg-white"
                }`}
              >
                {busy ? (
                  <Clock className="size-3.5" strokeWidth={2} />
                ) : (
                  <ArrowUp className="size-3.5" strokeWidth={2} />
                )}
              </button>
            </div>
          </div>
        </div>
      </div>
    </footer>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// Bottom bar (model / runtime / thinking / workspace pickers)

interface BottomBarProps {
  model: string;
  onModel: (m: string) => void;
  runtime: RuntimeMode;
  onRuntime: (r: RuntimeMode) => void;
  thinking: ThinkingMode;
  onThinking: (k: ThinkingMode) => void;
  disabled: boolean;
  envMode: EnvMode;
  onEnvMode: (m: EnvMode) => void;
  envTemplate: EnvTemplate | null;
  envStatus: EnvStatus | null;
  dotenvStatus: DotenvStatus | null;
  onEnvUp: () => void;
  onEnvDown: () => void;
  onRegenerateDotenv: () => void;
}

function BottomBar({
  model,
  onModel,
  runtime,
  onRuntime,
  thinking,
  onThinking,
  disabled,
  envMode,
  onEnvMode,
  envTemplate,
  envStatus,
  dotenvStatus,
  onEnvUp,
  onEnvDown,
  onRegenerateDotenv,
}: BottomBarProps) {
  const { t } = useTranslation("chat");
  return (
    <>
      <Chip
        icon={<Cpu className="size-3" strokeWidth={1.75} />}
        label={t("model")}
        disabled={disabled}
      >
        <select
          aria-label={t("model")}
          value={model}
          onChange={(e) => onModel(e.target.value)}
          disabled={disabled}
          className="bg-transparent text-neutral-200 outline-none disabled:opacity-60"
        >
          {MODELS.map((m) => (
            <option key={m || "auto"} value={m} className="bg-neutral-900">
              {m || t("model_auto")}
            </option>
          ))}
        </select>
      </Chip>

      <Chip
        icon={
          runtime === "plan" ? (
            <Sparkles className="size-3" strokeWidth={1.75} />
          ) : runtime === "full_access" ? (
            <PlayCircle className="size-3" strokeWidth={1.75} />
          ) : (
            <SquareIcon className="size-3" strokeWidth={1.75} />
          )
        }
        label={t("runtime")}
        disabled={disabled}
      >
        <select
          aria-label={t("runtime")}
          value={runtime}
          onChange={(e) => onRuntime(e.target.value as RuntimeMode)}
          disabled={disabled}
          className="bg-transparent text-neutral-200 outline-none disabled:opacity-60"
        >
          <option value="supervised" className="bg-neutral-900">
            {t("runtime_supervised")}
          </option>
          <option value="accept_edits" className="bg-neutral-900">
            {t("runtime_accept_edits")}
          </option>
          <option value="full_access" className="bg-neutral-900">
            {t("runtime_full_access")}
          </option>
          <option value="plan" className="bg-neutral-900">
            {t("runtime_plan")}
          </option>
        </select>
      </Chip>

      <Chip
        icon={<Brain className="size-3" strokeWidth={1.75} />}
        label={t("thinking")}
        disabled={disabled}
      >
        <select
          aria-label={t("thinking")}
          value={thinking}
          onChange={(e) => onThinking(e.target.value as ThinkingMode)}
          disabled={disabled}
          className="bg-transparent text-neutral-200 outline-none disabled:opacity-60"
        >
          <option value="auto" className="bg-neutral-900">
            {t("thinking_auto")}
          </option>
          <option value="on" className="bg-neutral-900">
            {t("thinking_on")}
          </option>
          <option value="off" className="bg-neutral-900">
            {t("thinking_off")}
          </option>
        </select>
      </Chip>

      {envTemplate?.has_template && (
        <Chip
          icon={
            envStatus?.up ? (
              <span className="size-1.5 rounded-full bg-emerald-400" />
            ) : (
              <span className="size-1.5 rounded-full bg-neutral-600" />
            )
          }
          label={t("env")}
        >
          <select
            aria-label={t("env")}
            value={envMode}
            onChange={(e) => onEnvMode(e.target.value as EnvMode)}
            className="bg-transparent text-neutral-200 outline-none"
          >
            <option value="default" className="bg-neutral-900">
              {t("env_default")}
            </option>
            <option value="worktree" className="bg-neutral-900">
              {t("env_worktree")}
            </option>
          </select>
          {envMode === "worktree" && (
            <>
              <button
                type="button"
                onClick={onEnvUp}
                aria-label={t("env_up")}
                title={t("env_up")}
                className="ml-0.5 flex size-3.5 items-center justify-center rounded text-emerald-400 hover:bg-emerald-950/40"
              >
                <PlayCircle className="size-3" strokeWidth={1.75} />
              </button>
              <button
                type="button"
                onClick={onEnvDown}
                aria-label={t("env_down")}
                title={t("env_down")}
                className="flex size-3.5 items-center justify-center rounded text-amber-400 hover:bg-amber-950/40"
              >
                <PauseCircle className="size-3" strokeWidth={1.75} />
              </button>
              {dotenvStatus?.has_template && (
                <button
                  type="button"
                  onClick={onRegenerateDotenv}
                  aria-label={t("env_dotenv_regenerate")}
                  title={
                    dotenvStatus.manual_override
                      ? t("env_dotenv_manual_override_short")
                      : dotenvStatus.stale
                        ? t("env_dotenv_stale")
                        : t("env_dotenv_regenerate")
                  }
                  className={`flex size-3.5 items-center justify-center rounded ${
                    dotenvStatus.manual_override
                      ? "text-neutral-500 hover:bg-neutral-800"
                      : dotenvStatus.stale
                        ? "text-amber-300 hover:bg-amber-950/40"
                        : "text-neutral-400 hover:bg-neutral-800"
                  }`}
                >
                  <Sparkles className="size-3" strokeWidth={1.75} />
                </button>
              )}
            </>
          )}
        </Chip>
      )}

      {disabled && (
        <span className="ml-1 text-[10px] text-neutral-600">
          {t("controls_locked_running")}
        </span>
      )}
    </>
  );
}

function Chip({
  icon,
  label,
  children,
  disabled,
}: {
  icon: React.ReactNode;
  label?: string;
  children: React.ReactNode;
  disabled?: boolean;
}) {
  return (
    <span
      title={label}
      className={`inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-[11px] ${
        disabled
          ? "border-neutral-800 text-neutral-500"
          : "border-neutral-800 text-neutral-300 hover:border-neutral-700"
      }`}
    >
      {icon}
      {children}
      <ChevronsUpDown className="size-2.5 text-neutral-500" strokeWidth={1.75} />
    </span>
  );
}

// ──────────────────────────────────────────────────────────────────────────
// Misc

function StatusBadge({ status }: { status: string }) {
  const { t } = useTranslation("chat");
  const label =
    status === "running"
      ? t("status_running")
      : status === "stopped"
        ? t("status_stopped")
        : status === "errored"
          ? t("status_errored")
          : t("status_idle");
  const dot =
    status === "running"
      ? "bg-emerald-400"
      : status === "errored"
        ? "bg-red-400"
        : "bg-neutral-600";
  return (
    <span className="inline-flex items-center gap-1 text-[11px] text-neutral-400">
      <span className={`inline-block size-1.5 rounded-full ${dot}`} />
      {label}
    </span>
  );
}

function getBusyTurnId(turns: TurnEntry[]): string | null {
  const last = turns[turns.length - 1];
  return last && last.status === "streaming" ? last.id : null;
}

function extractError(e: unknown): string {
  if (!e) return "unknown error";
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (typeof e === "object" && e && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  return JSON.stringify(e);
}
