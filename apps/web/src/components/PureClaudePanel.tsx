import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
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
  type WorktreeRow,
  worktreeList,
} from "~/ipc/worktree.ts";
import {
  claudePtySpawn,
  claudePureRefreshTitle,
  terminalList,
  terminalWrite,
} from "~/ipc/terminal.ts";
import { attachmentSave, blobToBase64 } from "~/ipc/attachments.ts";
import {
  playTurnCompleteChime,
  shouldNotify,
} from "~/lib/notificationSound.ts";
import {
  toSpeechLocale,
  useSpeechRecognition,
} from "~/hooks/useSpeechRecognition.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { TerminalView } from "~/components/TerminalPanel.tsx";

// Strip ANSI escape sequences (CSI + OSC) from raw PTY bytes so prompt text
// matches across redraws. Char-code based to keep raw control bytes out of
// source. ESC = 27, BEL = 7.
function stripAnsi(s: string): string {
  let out = "";
  for (let i = 0; i < s.length; i += 1) {
    if (s.charCodeAt(i) !== 27) {
      out += s[i];
      continue;
    }
    const next = s[i + 1];
    if (next === "[") {
      // CSI: skip until a final byte in @–~ (0x40–0x7e).
      i += 2;
      while (i < s.length) {
        const c = s.charCodeAt(i);
        if (c >= 0x40 && c <= 0x7e) break;
        i += 1;
      }
    } else if (next === "]") {
      // OSC: skip until BEL or ESC (start of the ST terminator).
      i += 2;
      while (i < s.length && s.charCodeAt(i) !== 7 && s.charCodeAt(i) !== 27) {
        i += 1;
      }
    } else {
      // Lone ESC or a 2-byte escape — drop ESC and the following byte.
      i += 1;
    }
  }
  return out;
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
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  // Attachments shown as chips ([Image #N] / [File #N]); the raw `@path` is
  // only assembled at send time, never shown in the textarea.
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const ensuredRef = useRef<string | null>(null);

  // "Agent done" chime for pure mode. There's no turn event stream to hook
  // (that's structured mode) — the claude TUI is opaque bytes — so we use an
  // output-idle heuristic: once the user submits a prompt we arm, and the
  // first stretch of PTY silence after live output declares the turn done.
  // Only chimes when the window is unfocused, so an occasional early fire
  // (e.g. a long quiet tool run) is harmless. Mirrors ChatPanel's chime.
  const IDLE_DONE_MS = 2500;
  const armedRef = useRef(false);
  const idleTimerRef = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(idleTimerRef.current), []);

  // Permission / input-request chime. The claude TUI renders a numbered menu
  // ("Do you want to proceed?" + "❯ 1. Yes" / "…don't ask again") whenever it
  // needs the user to approve a tool or answer a question. We sniff the raw PTY
  // bytes for that menu and ring once, immediately, when the window is
  // unfocused — separate from the done-idle chime below. A rolling, ANSI-
  // stripped tail handles the prompt being split across output chunks.
  const PROMPT_RE =
    /(do you want to (proceed|make this edit|create|run|continue))|(❯\s*\d+\.\s*yes)|(yes, and don'?t ask again)|(no, and tell claude)/i;
  const outTailRef = useRef("");
  // Latched true while a prompt is on screen so we chime once per request, not
  // once per redraw. Cleared when the user submits a response (see onPtyInput).
  const promptOpenRef = useRef(false);

  const detectPrompt = useCallback((data: string) => {
    const stripped = stripAnsi(data);
    const tail = (outTailRef.current + stripped).slice(-2000);
    outTailRef.current = tail;
    if (PROMPT_RE.test(tail) && !promptOpenRef.current) {
      promptOpenRef.current = true;
      if (shouldNotify()) playTurnCompleteChime();
    }
  }, []);

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

  // One attempt shortly after mount catches resumed sessions whose transcript
  // already exists from a previous run.
  useEffect(() => {
    const id = window.setTimeout(refreshTitle, 3000);
    return () => window.clearTimeout(id);
  }, [refreshTitle]);

  const onPtyInput = useCallback((data: string) => {
    // A submit from the user (Enter, unless Shift held for a newline) arms the
    // done-detector. Other keystrokes (navigation, autocomplete) don't.
    if (data.includes("\r")) {
      armedRef.current = true;
      // The user just answered a prompt (or started a turn): release the latch
      // and reset the sniff buffer so the next request can chime again.
      promptOpenRef.current = false;
      outTailRef.current = "";
    }
  }, []);

  const onPtyOutput = useCallback(
    (data: string) => {
      detectPrompt(data);
      if (!armedRef.current) return;
      window.clearTimeout(idleTimerRef.current);
      idleTimerRef.current = window.setTimeout(() => {
        armedRef.current = false;
        // If a permission/input prompt is on screen the prompt chime already
        // rang — don't double-ring as a "turn done".
        if (!promptOpenRef.current && shouldNotify()) playTurnCompleteChime();
        // Turn settled → claude has flushed the user message (and maybe a
        // summary) to its transcript; try to title from it.
        refreshTitle();
      }, IDLE_DONE_MS);
    },
    [refreshTitle, detectPrompt],
  );

  // Ensure exactly one claude PTY exists for this session: reuse an existing
  // one (survives remounts) or spawn a fresh one. Deduped via `ensuredRef`
  // rather than a cancel flag — under StrictMode the effect runs twice, and a
  // cancel flag would abort the only run that spawns. Setting state after a
  // real unmount is a harmless no-op in React 18.
  useEffect(() => {
    if (ensuredRef.current === sessionId) return;
    ensuredRef.current = sessionId;
    void (async () => {
      try {
        // Only ever reuse THIS session's claude PTY — never a shell. The dock
        // spawns `kind: "shell"` PTYs against the same session id, and
        // `terminalList` returns every kind in arbitrary (HashMap) order, so
        // grabbing `existing[0]` blindly could mount a plain terminal here
        // instead of the claude TUI.
        const existing = await terminalList({ session_id: sessionId });
        const claudePty = existing.find((tinfo) => tinfo.kind === "claude");
        if (claudePty) {
          setTermId(claudePty.id);
          return;
        }
        const info = await claudePtySpawn({
          session_id: sessionId,
          cols: 80,
          rows: 24,
        });
        setTermId(info.id);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        // Allow a retry on the next mount/session change.
        ensuredRef.current = null;
      }
    })();
  }, [sessionId]);

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

  const submit = () => {
    const trimmed = text.trim();
    if (!trimmed && attachments.length === 0) return;
    // Assemble the real claude input: each `@path` followed by a space, which
    // accepts claude's file autocomplete and closes the popup, THEN the text,
    // THEN a single Enter to submit. Sending `@path\r` alone only picks the
    // autocomplete entry without submitting — the bug the chips UX fixes.
    const refs = attachments.map((a) => `@${a.path} `).join("");
    armedRef.current = true; // arm the done-chime detector
    promptOpenRef.current = false; // fresh turn → re-arm the prompt chime
    outTailRef.current = "";
    sendToPty(`${refs}${trimmed}`);
    setText("");
    setAttachments([]);
  };

  // Auto-submit when a Ctrl+click-armed dictation ends (listening true→false).
  useEffect(() => {
    const wasListening = prevListeningRef.current;
    prevListeningRef.current = speech.listening;
    if (wasListening && !speech.listening && autoSubmitOnEndRef.current) {
      autoSubmitOnEndRef.current = false;
      setAutoSubmitArmed(false);
      submit();
    }
  }, [speech.listening, submit]);

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
  // inject the `@path ` ref straight into claude's live prompt — the trailing
  // space accepts claude's autocomplete and closes the popup (same trick as
  // `submit`). No chip: the ref lands in the TUI where the user can keep typing.
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
          void terminalWrite({ id: termId, data: `@${info.path} ` }).catch(
            () => {},
          );
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
          {onToggleTerminal && (
            <button
              type="button"
              onClick={onToggleTerminal}
              aria-label={t("terminal_heading")}
              title={t("terminal_heading")}
              className={`ml-auto flex size-6 items-center justify-center rounded transition ${
                terminalOpen
                  ? "bg-neutral-800 text-neutral-100"
                  : "text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
              }`}
            >
              <SquareTerminal className="size-3.5" strokeWidth={1.75} />
            </button>
          )}
        </header>
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
            onImagePaste={onTerminalImagePaste}
            onInput={onPtyInput}
            onOutput={onPtyOutput}
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
          value={text + (speech.interim ? ` ${speech.interim}` : "")}
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
          className="max-h-32 min-h-[34px] flex-1 resize-none rounded border border-neutral-800 bg-neutral-950 px-2 py-1.5 text-[12px] text-neutral-200 outline-none focus:border-neutral-700"
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
    </section>
  );
}
