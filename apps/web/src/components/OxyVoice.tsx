import { useCallback, useEffect, useRef } from "react";
import type { ProjectRow } from "~/ipc/commands.ts";
import { claudeLanguageDirective } from "~/lib/claudeLanguage.ts";
import { useAppSettingsStore } from "~/stores/appSettingsStore.ts";
import { useOxyStore } from "~/stores/oxyStore.ts";
import {
  sessionResume,
  sessionSendMessage,
  sessionStart,
} from "~/ipc/session.ts";
import { stripVoiceSubmitCommand } from "~/hooks/useSpeechRecognition.ts";
import { onOxyCommand, onOxyWake, voiceSpeak } from "~/ipc/voice.ts";
import { useSessionStore } from "~/stores/sessionStore.ts";

/**
 * Headless voice driver for Oxy. Always mounted. The backend owns the whole
 * audio pipeline (sherpa: wake-word KWS → whisper STT with energy endpointing),
 * so this component is now thin: on `oxy:wake` it opens the dock and shows the
 * listening state; on `oxy:command` it sends the transcribed text into the Oxy
 * session. No Web Speech, no mic juggling. See docs/design/oxy-assistant.md.
 */
export function OxyVoice({ project }: { project: ProjectRow | null }) {
  const setOpen = useOxyStore((s) => s.setOpen);
  const setSessionId = useOxyStore((s) => s.setSessionId);
  const setListening = useOxyStore((s) => s.setListening);
  const oxySessionId = useOxyStore((s) => s.sessionId);
  const oxySnapshot = useSessionStore((s) =>
    oxySessionId ? s.snapshots[oxySessionId] : undefined,
  );
  const spokenTurnRef = useRef<string | null>(null);

  const projectRef = useRef(project);
  useEffect(() => {
    projectRef.current = project;
  }, [project]);

  /** Resolve the Oxy session id, creating the session on first use. */
  const ensureSession = useCallback(async (): Promise<string | null> => {
    const cur = useOxyStore.getState().sessionId;
    if (cur) return cur;
    const p = projectRef.current;
    if (!p) return null;
    const res = await sessionStart({
      project_id: p.id,
      provider_id: "claude",
      environment: p.environment,
      cwd: p.root_path,
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
    return res.session_id;
  }, [setSessionId]);

  const send = useCallback(
    async (text: string) => {
      const id = await ensureSession();
      if (!id) return;
      try {
        await sessionSendMessage({ session_id: id, text });
      } catch {
        try {
          await sessionResume({ session_id: id });
          await sessionSendMessage({ session_id: id, text });
        } catch {
          /* the dock surfaces errors visually */
        }
      }
    },
    [ensureSession],
  );

  // Speak Oxy's reply aloud when a turn completes (if TTS is enabled). Reads the
  // live snapshot the dock's ChatPanel keeps updated.
  useEffect(() => {
    if (!useOxyStore.getState().ttsEnabled) return;
    const turns = oxySnapshot?.turns;
    if (!turns || turns.length === 0) return;
    const last = turns[turns.length - 1];
    if (!last || last.status !== "completed") return;
    if (spokenTurnRef.current === last.id) return;
    spokenTurnRef.current = last.id;
    const text = last.blocks
      .filter((b): b is { kind: "text"; text: string } => b.kind === "text")
      .map((b) => b.text)
      .join(" ")
      .trim();
    if (text) {
      const s = useOxyStore.getState();
      void voiceSpeak({
        text,
        sid: s.voiceSid,
        lang: s.voiceLang,
      }).catch(() => {});
    }
  }, [oxySnapshot]);

  useEffect(() => {
    // Guard against the async-listen leak: under StrictMode the cleanup can run
    // before `listen()` resolves, orphaning the first listener and stacking a
    // second — which fires every command twice. Track cancellation and unlisten
    // immediately if we already tore down.
    let cancelled = false;
    let unWake: (() => void) | undefined;
    let unCmd: (() => void) | undefined;
    void onOxyWake(() => {
      setOpen(true);
      setListening(true);
      void ensureSession();
    }).then((u) => {
      if (cancelled) u();
      else unWake = u;
    });
    void onOxyCommand((raw) => {
      setListening(false);
      const { text } = stripVoiceSubmitCommand(raw);
      if (text.trim()) void send(text.trim());
    }).then((u) => {
      if (cancelled) u();
      else unCmd = u;
    });
    return () => {
      cancelled = true;
      unWake?.();
      unCmd?.();
    };
  }, [setOpen, setListening, ensureSession, send]);

  return null;
}
