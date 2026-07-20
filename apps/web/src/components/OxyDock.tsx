import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Sparkles, SquarePen, X } from "lucide-react";
import type { ProjectRow } from "~/ipc/commands.ts";
import { sessionStart, sessionStop } from "~/ipc/session.ts";
import { claudeLanguageDirective } from "~/lib/claudeLanguage.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import { useOxyStore } from "~/stores/oxyStore.ts";
import { ChatPanel } from "~/components/ChatPanel.tsx";

/**
 * The always-available Oxy dock: a resizable right-side rail hosting the one
 * global assistant session. Rendered across every tab so Oxy is reachable from
 * anywhere. Structured only (never the pure PTY) and `embedded` so it never
 * spawns/hijacks a project session — its reply text stays clean for the voice
 * layer (see docs/design/oxy-assistant.md).
 */
export function OxyDock({ project }: { project: ProjectRow | null }) {
  const { t } = useTranslation("common");
  const open = useOxyStore((s) => s.open);
  const sessionId = useOxyStore((s) => s.sessionId);
  const setSessionId = useOxyStore((s) => s.setSessionId);
  const setOpen = useOxyStore((s) => s.setOpen);
  const width = useOxyStore((s) => s.width);
  const setWidth = useOxyStore((s) => s.setWidth);
  const listening = useOxyStore((s) => s.listening);
  const interim = useOxyStore((s) => s.interim);
  const [starting, setStarting] = useState(false);
  // Guard against double-spawn from the effect firing twice (StrictMode / rapid
  // clicks) before the session id lands in the store.
  const spawning = useRef(false);

  // Lazily create the single Oxy session the first time the dock opens with a
  // project available. Its own cwd is the active project's root, but its
  // cross-thread tools reach every project regardless.
  useEffect(() => {
    if (!open || sessionId || !project || spawning.current) return;
    spawning.current = true;
    setStarting(true);
    void (async () => {
      try {
        const res = await sessionStart({
          project_id: project.id,
          provider_id: "claude",
          environment: project.environment,
          cwd: project.root_path,
          model: "",
          thinking: "auto",
          runtime: useAppSettingsStore.getState().defaultRuntime,
          env_mode: "default",
          kind: "assistant",
          system_prompt: claudeLanguageDirective(
            useAppSettingsStore.getState().claudeLanguage,
          ),
        });
        setSessionId(res.session_id);
      } catch {
        spawning.current = false;
      } finally {
        setStarting(false);
      }
    })();
  }, [open, sessionId, project, setSessionId]);

  // "New conversation": stop the current Oxy session and drop its id. The effect
  // above then spawns a fresh one. Also the escape hatch for a stuck turn.
  const newConversation = useCallback(() => {
    const id = useOxyStore.getState().sessionId;
    spawning.current = false;
    setSessionId(null);
    if (id) void sessionStop({ session_id: id }).catch(() => {});
  }, [setSessionId]);

  // Left-edge drag to resize. Dock is right-anchored, so dragging left grows it.
  const onResizeStart = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startW = useOxyStore.getState().width;
      const onMove = (ev: MouseEvent) => setWidth(startW + (startX - ev.clientX));
      const onUp = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        document.body.style.userSelect = "";
      };
      document.body.style.userSelect = "none";
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [setWidth],
  );

  if (!open) return null;

  return (
    <div
      className="relative flex shrink-0 flex-col border-l border-neutral-800 bg-neutral-950"
      style={{ width }}
    >
      {/* Resize handle — sits on the left border. */}
      <div
        onMouseDown={onResizeStart}
        role="separator"
        aria-orientation="vertical"
        className="group absolute left-0 top-0 z-10 h-full w-1 -translate-x-1/2 cursor-col-resize"
      >
        <div className="h-full w-full bg-transparent transition group-hover:bg-emerald-700/50" />
      </div>

      {/* Single control bar: brand + new + close. */}
      <div className="flex items-center gap-1 border-b border-neutral-800 px-2 py-1">
        <Sparkles size={13} className="text-emerald-400" />
        <div className="flex-1" />
        <button
          type="button"
          onClick={newConversation}
          title={t("oxy.new_session")}
          className="rounded p-1 text-neutral-500 hover:bg-neutral-900 hover:text-neutral-300"
        >
          <SquarePen size={14} />
        </button>
        <button
          type="button"
          onClick={() => setOpen(false)}
          title={t("oxy.close")}
          className="rounded p-1 text-neutral-500 hover:bg-neutral-900 hover:text-neutral-300"
        >
          <X size={14} />
        </button>
      </div>

      {/* Listening overlay — floats just above the chat input. */}
      {listening && (
        <div className="pointer-events-none absolute inset-x-3 bottom-[72px] z-20 flex items-center gap-2 rounded-lg border border-emerald-700/60 bg-emerald-950/90 px-3 py-2 text-[12px] text-emerald-200 shadow-lg backdrop-blur">
          <span className="relative flex size-2.5 shrink-0">
            <span className="absolute inline-flex size-full animate-ping rounded-full bg-emerald-400 opacity-70" />
            <span className="relative inline-flex size-2.5 rounded-full bg-emerald-400" />
          </span>
          <span className="min-w-0 flex-1 truncate">
            {interim.trim() ? interim : t("oxy.listening_hint")}
          </span>
        </div>
      )}

      <div className="flex min-h-0 flex-1 flex-col">
        {!project ? (
          <div className="flex flex-1 items-center justify-center p-4 text-center text-[12px] text-neutral-500">
            {t("oxy.no_project")}
          </div>
        ) : starting || !sessionId ? (
          <div className="flex flex-1 items-center justify-center p-4 text-[12px] text-neutral-500">
            {t("oxy.starting")}
          </div>
        ) : (
          <ChatPanel
            key={sessionId}
            project={project}
            sessionId={sessionId}
            embedded
          />
        )}
      </div>
    </div>
  );
}
