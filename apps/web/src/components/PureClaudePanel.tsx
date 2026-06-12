import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Bot,
  FileText,
  Image as ImageIcon,
  Mic,
  MicOff,
  Paperclip,
  Send,
  SquareTerminal,
  Terminal as TerminalIcon,
  X,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { ProjectRow } from "~/ipc/commands.ts";
import {
  type RuntimeMode,
  sessionStart,
} from "~/ipc/session.ts";
import {
  isPrimaryWorktreeId,
  PRIMARY_WORKTREE_ID,
  type WorktreeRow,
  worktreeList,
} from "~/ipc/worktree.ts";
import { fsOpenExternal } from "~/ipc/fs.ts";
import {
  claudePtySpawn,
  claudePureRefreshTitle,
  onPureState,
  terminalKill,
  terminalList,
  terminalWrite,
} from "~/ipc/terminal.ts";
import { attachmentSave, blobToBase64 } from "~/ipc/attachments.ts";
import { playEscalationChime } from "~/lib/notificationSound.ts";
import { claudeLanguageDirective } from "~/lib/claudeLanguage.ts";
import { useBusyStore } from "~/stores/busyStore.ts";
import {
  toSpeechLocale,
  useSpeechRecognition,
} from "~/hooks/useSpeechRecognition.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import { TerminalView } from "~/components/TerminalPanel.tsx";
import { FileViewerModal } from "~/components/FileViewerModal.tsx";
import { AutopilotPanel } from "~/components/AutopilotPanel.tsx";
import { useAutopilotStore } from "~/stores/autopilotStore.ts";
import { useAutopilotAlertStore } from "~/stores/autopilotAlertStore.ts";
import { onAutopilotEvent } from "~/ipc/autopilot.ts";

/**
 * Turn a path token clicked in the terminal into a worktree-relative path the
 * fs IPC accepts (it rejects absolute paths and `..`). `cwd` is the PTY's
 * working directory, which equals the worktree root for pure sessions. Returns
 * null when the path escapes the worktree (can't be opened via worktree-scoped
 * fs ops) or can't be resolved.
 */
function resolveRelPath(raw: string, cwd: string): string | null {
  // Drop a trailing :line[:col] locator, then normalize separators.
  let p = raw.replace(/:\d+(?::\d+)?$/, "").replace(/\\/g, "/");
  let base = cwd.replace(/\\/g, "/").replace(/\/+$/, "");
  // A WSL UNC cwd (\\wsl.localhost\<distro>\home\…) maps to the POSIX path the
  // distro — and claude's output — actually use.
  const unc = base.match(/^\/\/(?:wsl\.localhost|wsl\$)\/[^/]+(.*)$/);
  if (unc) base = unc[1] || "/";

  const isAbs = /^[A-Za-z]:\//.test(p) || p.startsWith("/");
  if (isAbs) {
    const pl = p.toLowerCase();
    const bl = base.toLowerCase();
    if (pl === bl) return null; // the worktree root itself, not a file
    if (pl.startsWith(`${bl}/`)) return p.slice(base.length + 1);
    return null; // outside the worktree
  }
  // Relative → already worktree-root-relative (cwd == root). Reject escapes.
  p = p.replace(/^\.\//, "");
  if (p === ".." || p.startsWith("../")) return null;
  return p || null;
}

interface Attachment {
  id: string;
  /** Path as claude should see it (POSIX inside WSL). */
  path: string;
  isImage: boolean;
}

interface Props {
  project: ProjectRow | null;
  /**
   * When set, render this session directly (skipping the start picker) — used
   * to embed a pure session as a Multi View pane. When absent, the panel
   * follows the global active session.
   */
  sessionId?: string;
  /** Reports the pane's claude PTY id once spawned (for broadcast). */
  onPtyReady?: (ptyId: string) => void;
  /** Hide the panel's own header (the embedding pane provides its own). */
  embedded?: boolean;
  /** Toggle the session's shell terminal dock (non-embedded only). */
  onToggleTerminal?: (() => void) | undefined;
  /** Whether the shell terminal dock is currently open. */
  terminalOpen?: boolean;
}

/**
 * "Claude Code puro" — the interactive `claude` TUI hosted in a PTY, replacing
 * the structured chat surface when pure mode is enabled. The xterm is the
 * primary interface (type straight into it); the composer bar below is a
 * convenience for voice dictation and attachments, both of which inject text
 * into the same PTY stdin.
 */
export function PureClaudePanel({
  project,
  sessionId,
  onPtyReady,
  embedded,
  onToggleTerminal,
  terminalOpen,
}: Props) {
  const { t, i18n } = useTranslation("chat");
  const activeId = useSessionStore((s) => s.activeSessionId);
  const setActive = useSessionStore((s) => s.setActive);

  if (!project) {
    return (
      <div className="flex h-full items-center justify-center text-[12px] text-neutral-500">
        {t("pure_no_project")}
      </div>
    );
  }

  // Embedded (Multi View pane): a fixed session, no start picker, no header.
  if (sessionId) {
    return (
      <PureSessionView
        key={sessionId}
        sessionId={sessionId}
        project={project}
        i18nLang={i18n.language}
        onPtyReady={onPtyReady}
        embedded={embedded}
      />
    );
  }

  return activeId ? (
    <PureSessionView
      key={activeId}
      sessionId={activeId}
      project={project}
      i18nLang={i18n.language}
      onToggleTerminal={onToggleTerminal}
      terminalOpen={terminalOpen}
    />
  ) : (
    <PureStartView project={project} onStarted={setActive} />
  );
}

// ── Start view: pick worktree/model/runtime, then create a pure session ──────

function PureStartView({
  project,
  onStarted,
}: {
  project: ProjectRow;
  onStarted: (id: string) => void;
}) {
  const { t } = useTranslation("chat");
  const [worktrees, setWorktrees] = useState<WorktreeRow[]>([]);
  const [worktreeId, setWorktreeId] = useState<string>("");
  const [model, setModel] = useState<string>("");
  const [runtime, setRuntime] = useState<RuntimeMode>("supervised");
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void worktreeList({ project_id: project.id })
      .then((rows) => {
        if (!cancelled) setWorktrees(rows);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [project.id]);

  const start = useCallback(async () => {
    setStarting(true);
    setError(null);
    try {
      const wt = worktreeId
        ? worktrees.find((w) => w.id === worktreeId) ?? null
        : null;
      const wtIdToSend =
        wt && !isPrimaryWorktreeId(wt.id) ? wt.id : undefined;
      const res = await sessionStart({
        project_id: project.id,
        provider_id: "claude",
        environment: project.environment,
        cwd: wt ? wt.path : project.root_path,
        model,
        runtime,
        env_mode: "default",
        kind: "pure",
        ...(wtIdToSend ? { worktree_id: wtIdToSend } : {}),
      });
      onStarted(res.session_id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setStarting(false);
    }
  }, [project, worktrees, worktreeId, model, runtime, onStarted]);

  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="w-full max-w-md rounded-xl border border-neutral-800 bg-neutral-900/50 p-5">
        <h2 className="mb-1 flex items-center gap-2 text-sm font-medium text-neutral-100">
          <TerminalIcon className="size-4" strokeWidth={1.75} />
          {t("pure_start_title")}
        </h2>
        <p className="mb-4 text-[11px] text-neutral-500">
          {t("pure_start_desc")}
        </p>

        <label className="mb-3 block">
          <span className="mb-1 block text-[11px] text-neutral-400">
            {t("pure_workspace_label")}
          </span>
          <select
            value={worktreeId}
            onChange={(e) => setWorktreeId(e.target.value)}
            className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700"
          >
            <option value="" className="bg-neutral-900">
              {t("pure_workspace_root")}
            </option>
            {worktrees
              .filter((w) => !w.is_primary && !w.removed_at)
              .map((w) => (
                <option key={w.id} value={w.id} className="bg-neutral-900">
                  {w.name} ({w.branch})
                </option>
              ))}
          </select>
        </label>

        <label className="mb-3 block">
          <span className="mb-1 block text-[11px] text-neutral-400">
            {t("pure_model_label")}
          </span>
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder={t("pure_model_placeholder")}
            className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700"
          />
        </label>

        <label className="mb-4 block">
          <span className="mb-1 block text-[11px] text-neutral-400">
            {t("pure_runtime_label")}
          </span>
          <select
            value={runtime}
            onChange={(e) => setRuntime(e.target.value as RuntimeMode)}
            className="w-full rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700"
          >
            <option value="supervised" className="bg-neutral-900">
              {t("pure_runtime_supervised")}
            </option>
            <option value="accept_edits" className="bg-neutral-900">
              {t("pure_runtime_accept_edits")}
            </option>
            <option value="full_access" className="bg-neutral-900">
              {t("pure_runtime_full_access")}
            </option>
            <option value="plan" className="bg-neutral-900">
              {t("pure_runtime_plan")}
            </option>
          </select>
        </label>

        {error && (
          <p className="mb-3 rounded border border-red-900/60 bg-red-950/30 px-2 py-1 text-[11px] text-red-200">
            {error}
          </p>
        )}

        <button
          type="button"
          onClick={() => void start()}
          disabled={starting}
          className="w-full rounded bg-neutral-200 px-3 py-1.5 text-[12px] font-medium text-neutral-900 hover:bg-white disabled:opacity-50"
        >
          {starting ? t("pure_starting") : t("pure_start_button")}
        </button>
      </div>
    </div>
  );
}

// ── Session view: claude PTY + composer bar ──────────────────────────────────

function PureSessionView({
  sessionId,
  project,
  i18nLang,
  onPtyReady,
  embedded,
  onToggleTerminal,
  terminalOpen,
}: {
  sessionId: string;
  project: ProjectRow;
  i18nLang: string;
  onPtyReady?: ((ptyId: string) => void) | undefined;
  embedded?: boolean | undefined;
  onToggleTerminal?: (() => void) | undefined;
  terminalOpen?: boolean | undefined;
}) {
  const { t } = useTranslation("chat");
  const [termId, setTermId] = useState<string | null>(null);
  const [cwd, setCwd] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  // Ctrl/Cmd+click target when opening a terminal file path in the in-app modal.
  const [openRelPath, setOpenRelPath] = useState<string | null>(null);
  const openFilesExternally = useAppSettingsStore((s) => s.openFilesExternally);
  // The fs ops a file-open uses must be scoped to the SESSION's own project +
  // worktree, not whatever project is selected in the sidebar (`project`). A
  // pure session can run against a different project than the active one — using
  // `project.id` joined the session's relative path onto the wrong root (the
  // "Windows cannot find …" error). Fall back to the prop only if the snapshot
  // hasn't hydrated yet.
  const setBusy = useBusyStore((s) => s.setBusy);
  const setNeedsInput = useSessionStore((s) => s.setNeedsInput);
  const openProjectId = useSessionStore(
    (s) => s.snapshots[sessionId]?.project_id ?? project.id,
  );
  const worktreeId = useSessionStore(
    (s) => s.snapshots[sessionId]?.worktree_id ?? PRIMARY_WORKTREE_ID,
  );
  // Attachments shown as chips ([Image #N] / [File #N]); the raw `@path` is
  // only assembled at send time, never shown in the textarea.
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const ensuredRef = useRef<string | null>(null);

  // Auto-pilot: floating mission panel + engaged indicator on the header button.
  const [autopilotOpen, setAutopilotOpen] = useState(false);
  const autopilotHydrate = useAutopilotStore((s) => s.hydrate);
  const autopilotOn = useAutopilotStore((s) => s.enabled[sessionId] ?? false);
  const autopilotPushLog = useAutopilotStore((s) => s.pushLog);
  const autopilotSetEnabled = useAutopilotStore((s) => s.setEnabled);
  const autopilotSetThinking = useAutopilotStore((s) => s.setThinking);
  useEffect(() => {
    autopilotHydrate(sessionId);
  }, [sessionId, autopilotHydrate]);

  // Stream the backend pilot's decisions into the store log. Lives here (not the
  // panel) so the log accumulates and a Halt auto-disengages even with the panel
  // closed. The backend already stopped driving on Halt; we just sync UI state.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void onAutopilotEvent(sessionId, (event) => {
      // "thinking" is transient feedback, not a decision — flip the flag, don't
      // clutter the log. Any other event means the step resolved → clear it.
      if (event.kind === "thinking") {
        autopilotSetThinking(sessionId, true);
        return;
      }
      autopilotSetThinking(sessionId, false);
      autopilotPushLog(sessionId, event);
      if (
        event.kind === "halted" ||
        event.kind === "error" ||
        event.kind === "escalated"
      ) {
        autopilotSetEnabled(sessionId, false);
      }
      // Escalation = "I can't do this, you have to" — alert hard: a distinct
      // chime (regardless of focus) + a balloon carrying the explanation.
      if (event.kind === "escalated") {
        playEscalationChime();
        useAutopilotAlertStore.getState().raise(sessionId, event.why);
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [sessionId, autopilotPushLog, autopilotSetEnabled, autopilotSetThinking]);

  // Auto-recover a dead claude PTY. The interactive `claude` TUI exits on a
  // double Ctrl+C (and on any crash); without this the pane just freezes on
  // "[process exited]" and the user has to leave and re-enter the session. On
  // exit we respawn — and because `claude_pty_spawn` resumes from the existing
  // `<session-id>.jsonl` transcript, the conversation continues where it died.
  //
  // Crash-loop guard: if claude keeps dying within RESPAWN_MIN_MS of being
  // (re)spawned, count strikes; after RESPAWN_MAX consecutive fast deaths stop
  // respawning and surface an error instead of thrashing forever. A death after
  // a healthy run (≥ RESPAWN_MIN_MS uptime) resets the strike counter.
  const RESPAWN_MIN_MS = 4000;
  const RESPAWN_MAX = 3;
  const [spawnNonce, setSpawnNonce] = useState(0);
  const spawnAtRef = useRef(0);
  const respawnStrikesRef = useRef(0);
  const termIdRef = useRef<string | null>(null);
  useEffect(() => {
    termIdRef.current = termId;
  }, [termId]);

  const onPtyExit = useCallback(() => {
    const uptime = Date.now() - spawnAtRef.current;
    respawnStrikesRef.current =
      uptime < RESPAWN_MIN_MS ? respawnStrikesRef.current + 1 : 0;
    if (respawnStrikesRef.current > RESPAWN_MAX) {
      setError(t("pure_respawn_failed"));
      return;
    }
    // Purge the dead PTY from the backend registry — it lingers there after a
    // child exit (only `kill` removes it), so a plain remount would re-attach
    // to the corpse via `terminalList` instead of spawning fresh.
    const dead = termIdRef.current;
    if (dead) void terminalKill({ id: dead }).catch(() => {});
    ensuredRef.current = null;
    setTermId(null);
    setSpawnNonce((n) => n + 1);
  }, [t]);

  const taRef = useRef<HTMLTextAreaElement | null>(null);

  // Pure sessions get no auto-title from a turn-event stream. Instead we read
  // claude's own transcript (it's written under our `--session-id`) once a turn
  // settles. The backend no-ops if the session is already titled, so calling it
  // again is cheap and idempotent; stop once it sticks.
  const titleSetRef = useRef(false);
  const refreshTitle = useCallback(() => {
    if (titleSetRef.current) return;
    void claudePureRefreshTitle({ session_id: sessionId })
      .then((title) => {
        if (title) titleSetRef.current = true;
      })
      .catch(() => {});
  }, [sessionId]);

  // Dot state (busy / needs-input) and the chimes are owned by the single
  // pure-state bridge in `Sidebar` — this panel only watches the same backend
  // snapshot to auto-title on a turn settling (busy → done). Tracking the
  // transition avoids titling on a freshly-resumed idle session.
  const prevBusyRef = useRef(false);
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void onPureState(sessionId, (snap) => {
      const wasBusy = prevBusyRef.current;
      prevBusyRef.current = snap.busy;
      if (wasBusy && !snap.busy) refreshTitle();
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [sessionId, refreshTitle]);

  // One attempt shortly after mount catches resumed sessions whose transcript
  // already exists from a previous run.
  useEffect(() => {
    const id = window.setTimeout(refreshTitle, 3000);
    return () => window.clearTimeout(id);
  }, [refreshTitle]);

  const onPtyInput = useCallback((data: string) => {
    // A submit (Enter, unless Shift held for a newline) starts a turn. The
    // backend resets its latches + stamps its output clock on the `\r` and will
    // emit a fresh busy snapshot once claude paints its spinner; reflect the
    // turn start here immediately so the dot doesn't lag. Other keystrokes don't.
    if (data.includes("\r")) {
      setBusy(sessionId, true);
      setNeedsInput(sessionId, false);
    }
  }, [sessionId, setBusy, setNeedsInput]);

  // NB: no unmount-clear here. Leaving the chat tab unmounts this panel mid-turn
  // and clearing on unmount made the sidebar dot vanish while still working. The
  // idle clear lives at the App level instead (watches the PTY output and
  // survives tab switches); see App.tsx.

  // Ensure exactly one claude PTY exists for this session: reuse an existing
  // one (survives remounts) or spawn a fresh one. Deduped via `ensuredRef`
  // rather than a cancel flag — under StrictMode the effect runs twice, and a
  // cancel flag would abort the only run that spawns. Setting state after a
  // real unmount is a harmless no-op in React 18.
  useEffect(() => {
    if (ensuredRef.current === sessionId) return;
    ensuredRef.current = sessionId;
    // Capture the session this run targets. If `sessionId` changes while the
    // async work is in flight, `ensuredRef.current` moves to the new id and the
    // checks below abort, so a stale spawn can't mount the previous session's
    // PTY id into the new view. (Distinct from a cancel flag, which would also
    // abort the StrictMode re-run that actually spawns — guarding on the ref
    // keeps same-session re-runs alive.)
    const target = sessionId;
    void (async () => {
      try {
        // Only ever reuse THIS session's claude PTY — never a shell. The dock
        // spawns `kind: "shell"` PTYs against the same session id, and
        // `terminalList` returns every kind in arbitrary (HashMap) order, so
        // grabbing `existing[0]` blindly could mount a plain terminal here
        // instead of the claude TUI.
        const existing = await terminalList({ session_id: sessionId });
        if (ensuredRef.current !== target) return;
        const claudePty = existing.find((tinfo) => tinfo.kind === "claude");
        if (claudePty) {
          spawnAtRef.current = Date.now();
          setTermId(claudePty.id);
          setCwd(claudePty.cwd);
          return;
        }
        const info = await claudePtySpawn({
          session_id: sessionId,
          cols: 80,
          rows: 24,
          system_prompt: claudeLanguageDirective(
            useAppSettingsStore.getState().claudeLanguage,
          ),
        });
        if (ensuredRef.current !== target) return;
        spawnAtRef.current = Date.now();
        setTermId(info.id);
        setCwd(info.cwd);
      } catch (e) {
        if (ensuredRef.current !== target) return;
        setError(e instanceof Error ? e.message : String(e));
        // Allow a retry on the next mount/session change.
        ensuredRef.current = null;
      }
    })();
  }, [sessionId, spawnNonce]);

  // Surface the PTY id upward (Multi View broadcast targets it).
  useEffect(() => {
    if (termId) onPtyReady?.(termId);
  }, [termId, onPtyReady]);

  const sendToPty = useCallback(
    (value: string) => {
      if (!termId) return;
      const id = termId;
      // Send the text, then the Enter as a SEPARATE write after a short pause.
      // claude's TUI has paste-burst detection: text + "\r" in one write is
      // read as a paste, so the trailing carriage return becomes a literal
      // newline instead of submitting — the intermittent "didn't submit" bug.
      // A standalone "\r" arriving after the burst window submits reliably.
      void terminalWrite({ id, data: value })
        .then(() => new Promise((r) => setTimeout(r, 60)))
        .then(() => terminalWrite({ id, data: "\r" }))
        .catch(() => {});
    },
    [termId],
  );

  // Ctrl/Cmd+click on a file path in the TUI. Resolve it against the PTY cwd,
  // then either hand off to the external editor or pop the in-app modal.
  const onOpenPath = useCallback(
    (raw: string) => {
      if (!cwd) return;
      const rel = resolveRelPath(raw, cwd);
      if (!rel) return;
      if (openFilesExternally) {
        void fsOpenExternal({
          projectId: openProjectId,
          worktreeId,
          relPath: rel,
        }).catch((e) =>
          setError(e instanceof Error ? e.message : String(e)),
        );
      } else {
        setOpenRelPath(rel);
      }
    },
    [cwd, openFilesExternally, openProjectId, worktreeId],
  );

  const speech = useSpeechRecognition({
    lang: toSpeechLocale(i18nLang),
    onFinal: (chunk) => setText((prev) => (prev ? `${prev} ${chunk}` : chunk)),
  });

  // Ctrl/Cmd+click on the mic dictates then auto-submits to the PTY once
  // recognition ends. Armed via ref so it survives until `onend`.
  const autoSubmitOnEndRef = useRef(false);
  const [autoSubmitArmed, setAutoSubmitArmed] = useState(false);
  const prevListeningRef = useRef(false);

  const onMicClick = (e: React.MouseEvent) => {
    if (speech.listening) {
      speech.stop();
      return;
    }
    const auto = e.ctrlKey || e.metaKey;
    autoSubmitOnEndRef.current = auto;
    setAutoSubmitArmed(auto);
    speech.start();
  };

  const addAttachment = useCallback((path: string, isImage: boolean) => {
    setAttachments((prev) => [
      ...prev,
      { id: crypto.randomUUID?.() ?? `${Date.now()}-${prev.length}`, path, isImage },
    ]);
  }, []);

  const removeAttachment = (id: string) =>
    setAttachments((prev) => prev.filter((a) => a.id !== id));

  const submit = useCallback(() => {
    const trimmed = text.trim();
    if (!trimmed && attachments.length === 0) return;
    // Assemble the real claude input: each `@path` followed by a space, which
    // accepts claude's file autocomplete and closes the popup, THEN the text,
    // THEN a single Enter to submit. Sending `@path\r` alone only picks the
    // autocomplete entry without submitting — the bug the chips UX fixes.
    const refs = attachments.map((a) => `@${a.path} `).join("");
    // The trailing `\r` sendToPty emits drives the backend's latch reset;
    // reflect the turn start in the UI immediately. The backend owns detection.
    setBusy(sessionId, true); // sidebar pulse on
    sendToPty(`${refs}${trimmed}`);
    setText("");
    setAttachments([]);
  }, [text, attachments, sessionId, sendToPty, setBusy]);
  // Latest `submit` reachable from effects without making them a dep (which
  // would re-run / re-subscribe on every keystroke as text/attachments change).
  const submitRef = useRef(submit);
  submitRef.current = submit;

  // Auto-grow the composer with its content, capped (max-h-32 = 128px), and
  // collapse back to one row when cleared (submit / empty). Measures the
  // displayed value, which includes any interim dictation transcript.
  const composerValue = text + (speech.interim ? ` ${speech.interim}` : "");
  useEffect(() => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 128)}px`;
  }, [composerValue]);

  // Auto-submit when a Ctrl+click-armed dictation ends (listening true→false).
  // Deps are just `speech.listening` — `submit` is read via its ref so this
  // doesn't re-run on every keystroke.
  useEffect(() => {
    const wasListening = prevListeningRef.current;
    prevListeningRef.current = speech.listening;
    if (wasListening && !speech.listening && autoSubmitOnEndRef.current) {
      autoSubmitOnEndRef.current = false;
      setAutoSubmitArmed(false);
      submitRef.current();
    }
  }, [speech.listening]);

  // For WSL projects a picked file comes back as a `\\wsl.localhost\<distro>\…`
  // UNC path — claude runs inside the distro and needs the POSIX form.
  const toClaudeRef = (picked: string): string => {
    const m = picked.match(/^[\\/][\\/](?:wsl\.localhost|wsl\$)[\\/][^\\/]+(.*)$/);
    if (m) return (m[1] ?? "").replace(/\\/g, "/") || "/";
    return picked;
  };

  const pickAttachment = useCallback(async () => {
    try {
      const picked = await openDialog({
        multiple: false,
        defaultPath: project.root_path,
      });
      if (typeof picked !== "string") return;
      const isImage = /\.(png|jpe?g|gif|webp|bmp|svg)$/i.test(picked);
      addAttachment(toClaudeRef(picked), isImage);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [project.root_path, addAttachment]);

  // Image pasted (Ctrl/Cmd+V) directly into the xterm. Save the blob, then
  // inject the saved file's path into claude's live prompt wrapped in bracketed
  // paste markers (\e[200~ … \e[201~). claude runs its path→image detection
  // only on *pasted* text, never on typed text: a bare write landed the path as
  // literal characters (no conversion), whereas a bracketed paste makes claude
  // ingest the file and render it as `[Image #N]` — matching a native-terminal
  // paste. The trailing space sits OUTSIDE the markers so it's a normal keypress
  // separating the chip from whatever the user types next. No leading `@`: that
  // opened claude's file-autocomplete popup without consuming the path, leaving
  // a stale popup that hijacked the next `@` the user typed.
  const onTerminalImagePaste = useCallback(
    async (file: File) => {
      try {
        const base64 = await blobToBase64(file);
        const info = await attachmentSave({
          bucket_id: sessionId,
          mime: file.type,
          data_base64: base64,
          ...(file.name ? { filename: file.name } : {}),
        });
        if (termId) {
          void terminalWrite({
            id: termId,
            data: `\x1b[200~${info.path}\x1b[201~ `,
          }).catch(() => {});
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [sessionId, termId],
  );

  const onPaste = async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const items = e.clipboardData.items;
    if (!items) return;
    for (let i = 0; i < items.length; i += 1) {
      const item = items[i];
      if (item && item.kind === "file") {
        const file = item.getAsFile();
        if (!file) continue;
        e.preventDefault();
        try {
          const base64 = await blobToBase64(file);
          const info = await attachmentSave({
            bucket_id: sessionId,
            mime: file.type,
            data_base64: base64,
            ...(file.name ? { filename: file.name } : {}),
          });
          addAttachment(info.path, (file.type || "").startsWith("image/"));
        } catch (err) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }
    }
  };

  return (
    <section className="flex h-full min-h-0 flex-col bg-neutral-950">
      {!embedded && (
        <header className="flex items-center gap-2 border-b border-neutral-800 bg-neutral-900 px-3 py-1.5 text-[11px] text-neutral-300">
          <TerminalIcon className="size-3.5" strokeWidth={1.75} />
          <span className="font-medium">{t("pure_header")}</span>
          <span className="truncate text-neutral-500">· {project.name}</span>
          <div className="ml-auto flex items-center gap-1">
            <button
              type="button"
              onClick={() => setAutopilotOpen((v) => !v)}
              aria-label={t("autopilot_open")}
              title={autopilotOn ? t("autopilot_on") : t("autopilot_off")}
              className={`flex size-6 items-center justify-center rounded transition ${
                autopilotOpen
                  ? "bg-neutral-800 text-neutral-100"
                  : autopilotOn
                    ? "text-emerald-400 hover:bg-neutral-800"
                    : "text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
              }`}
            >
              <Bot className="size-3.5" strokeWidth={1.75} />
            </button>
            {onToggleTerminal && (
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
                <SquareTerminal className="size-3.5" strokeWidth={1.75} />
              </button>
            )}
          </div>
        </header>
      )}

      {autopilotOpen && (
        <AutopilotPanel
          sessionId={sessionId}
          onClose={() => setAutopilotOpen(false)}
        />
      )}

      {error && (
        <p className="border-b border-red-900/50 bg-red-950/30 px-3 py-1.5 text-[11px] text-red-200">
          {error}
        </p>
      )}

      <div className="relative min-h-0 flex-1 overflow-hidden">
        {termId ? (
          <TerminalView
            terminalId={termId}
            visible
            onExit={onPtyExit}
            onImagePaste={onTerminalImagePaste}
            onInput={onPtyInput}
            onOpenPath={onOpenPath}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-[11px] text-neutral-500">
            {t("pure_spawning")}
          </div>
        )}
      </div>

      <div className="flex flex-col gap-1.5 border-t border-neutral-800 bg-neutral-900 p-2">
        {attachments.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {attachments.map((a, i) => {
              const imageCount = attachments
                .slice(0, i + 1)
                .filter((x) => x.isImage).length;
              const fileCount = attachments
                .slice(0, i + 1)
                .filter((x) => !x.isImage).length;
              const label = a.isImage
                ? t("pure_chip_image", { n: imageCount })
                : t("pure_chip_file", { n: fileCount });
              return (
                <span
                  key={a.id}
                  title={a.path}
                  className="inline-flex items-center gap-1 rounded border border-neutral-700 bg-neutral-800/60 px-1.5 py-0.5 text-[11px] text-neutral-200"
                >
                  {a.isImage ? (
                    <ImageIcon className="size-3" strokeWidth={1.75} />
                  ) : (
                    <FileText className="size-3" strokeWidth={1.75} />
                  )}
                  {label}
                  <button
                    type="button"
                    onClick={() => removeAttachment(a.id)}
                    aria-label={t("pure_chip_remove")}
                    className="ml-0.5 text-neutral-500 hover:text-neutral-200"
                  >
                    <X className="size-3" strokeWidth={2} />
                  </button>
                </span>
              );
            })}
          </div>
        )}
        <div className="flex items-end gap-2">
        <textarea
          ref={taRef}
          value={composerValue}
          onChange={(e) => setText(e.target.value)}
          onPaste={(e) => void onPaste(e)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          rows={1}
          placeholder={t("pure_composer_placeholder")}
          className="max-h-32 min-h-[34px] flex-1 resize-none overflow-y-auto rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700"
        />
        {speech.supported && (
          <button
            type="button"
            onClick={onMicClick}
            aria-label={
              speech.listening
                ? autoSubmitArmed
                  ? t("speech_stop_autosubmit")
                  : t("voice_stop")
                : t("voice_start")
            }
            title={
              speech.listening
                ? autoSubmitArmed
                  ? t("speech_listening_autosubmit")
                  : t("voice_stop")
                : `${t("voice_start")} · ${t("speech_autosubmit_hint")}`
            }
            className={`flex size-[34px] items-center justify-center rounded border ${
              speech.listening
                ? autoSubmitArmed
                  ? "border-sky-700 bg-sky-950/40 text-sky-300"
                  : "border-red-700 bg-red-950/40 text-red-300"
                : "border-neutral-700 text-neutral-300 hover:bg-neutral-800"
            }`}
          >
            {speech.listening ? (
              <MicOff className="size-4" strokeWidth={1.75} />
            ) : (
              <Mic className="size-4" strokeWidth={1.75} />
            )}
          </button>
        )}
        <button
          type="button"
          onClick={() => void pickAttachment()}
          aria-label={t("pure_attach")}
          title={t("pure_attach_hint")}
          className="flex size-[34px] items-center justify-center rounded border border-neutral-700 text-neutral-300 hover:bg-neutral-800"
        >
          <Paperclip className="size-4" strokeWidth={1.75} />
        </button>
        <button
          type="button"
          onClick={submit}
          aria-label={t("send")}
          title={t("send")}
          className="flex size-[34px] items-center justify-center rounded bg-neutral-200 text-neutral-900 hover:bg-white"
        >
          <Send className="size-4" strokeWidth={1.75} />
        </button>
        </div>
      </div>

      {openRelPath && (
        <FileViewerModal
          projectId={openProjectId}
          worktreeId={worktreeId}
          relPath={openRelPath}
          onClose={() => setOpenRelPath(null)}
        />
      )}
    </section>
  );
}
