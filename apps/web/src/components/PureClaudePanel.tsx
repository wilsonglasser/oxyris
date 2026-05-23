import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  FileText,
  Image as ImageIcon,
  Mic,
  MicOff,
  Paperclip,
  Send,
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
  terminalList,
  terminalWrite,
} from "~/ipc/terminal.ts";
import { attachmentSave, blobToBase64 } from "~/ipc/attachments.ts";
import {
  toSpeechLocale,
  useSpeechRecognition,
} from "~/hooks/useSpeechRecognition.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";
import { TerminalView } from "~/components/TerminalPanel.tsx";

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
}: {
  sessionId: string;
  project: ProjectRow;
  i18nLang: string;
  onPtyReady?: ((ptyId: string) => void) | undefined;
  embedded?: boolean | undefined;
}) {
  const { t } = useTranslation("chat");
  const [termId, setTermId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  // Attachments shown as chips ([Image #N] / [File #N]); the raw `@path` is
  // only assembled at send time, never shown in the textarea.
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const ensuredRef = useRef<string | null>(null);

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
        const existing = await terminalList({ session_id: sessionId });
        if (existing.length > 0 && existing[0]) {
          setTermId(existing[0].id);
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
      // Carriage return submits the line to claude's prompt.
      void terminalWrite({ id: termId, data: `${value}\r` }).catch(() => {});
    },
    [termId],
  );

  const speech = useSpeechRecognition({
    lang: toSpeechLocale(i18nLang),
    onFinal: (chunk) => setText((prev) => (prev ? `${prev} ${chunk}` : chunk)),
  });

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
    sendToPty(`${refs}${trimmed}`);
    setText("");
    setAttachments([]);
  };

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
        </header>
      )}

      {error && (
        <p className="border-b border-red-900/50 bg-red-950/30 px-3 py-1.5 text-[11px] text-red-200">
          {error}
        </p>
      )}

      <div className="relative min-h-0 flex-1 overflow-hidden">
        {termId ? (
          <TerminalView terminalId={termId} visible />
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
            onClick={() => speech.toggle()}
            aria-label={speech.listening ? t("voice_stop") : t("voice_start")}
            title={speech.listening ? t("voice_stop") : t("voice_start")}
            className={`flex size-[34px] items-center justify-center rounded border ${
              speech.listening
                ? "border-red-700 bg-red-950/40 text-red-300"
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
