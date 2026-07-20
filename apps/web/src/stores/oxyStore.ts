import { create } from "zustand";

/**
 * Oxy — the single, app-global assistant. Unlike normal threads it is NOT tied
 * to the active project/session: it lives in a persistent right-side dock and
 * reaches every open thread through its cross-thread MCP tools. We keep just its
 * session id (so the same conversation survives reloads) and the dock's
 * open/closed state here.
 */
const STORAGE_KEY = "oxy.session_id";
const WIDTH_KEY = "oxy.width";
const WAKE_KEY = "oxy.wake_enabled";
const KEYWORD_KEY = "oxy.keyword";
const THRESHOLD_KEY = "oxy.threshold";
const DEVICE_KEY = "oxy.device";
const TTS_KEY = "oxy.tts_enabled";
const SID_KEY = "oxy.voice_sid";
const LANG_KEY = "oxy.voice_lang";
export const OXY_MIN_WIDTH = 300;
export const OXY_MAX_WIDTH = 820;

function initialWidth(): number {
  if (typeof localStorage === "undefined") return 380;
  const raw = Number(localStorage.getItem(WIDTH_KEY));
  if (!Number.isFinite(raw) || raw <= 0) return 380;
  return Math.min(OXY_MAX_WIDTH, Math.max(OXY_MIN_WIDTH, raw));
}

function ls(key: string, fallback: string): string {
  if (typeof localStorage === "undefined") return fallback;
  return localStorage.getItem(key) ?? fallback;
}

interface OxyState {
  /** The one Oxy assistant session, once created. Persisted across reloads. */
  sessionId: string | null;
  /** Whether the right-side Oxy dock is visible. */
  open: boolean;
  /** Dock width in px. Persisted, clamped to [MIN, MAX]. */
  width: number;
  // ── Voice (persisted config) ──────────────────────────────────────────────
  /** Whether the wake word is armed. Persisted so it survives restarts. */
  wakeEnabled: boolean;
  /** Wake keyword (plain text). */
  keyword: string;
  /** Detection threshold. */
  threshold: number;
  /** Input device name ("" = system default). */
  device: string;
  /** Whether Oxy speaks its replies aloud (Kokoro TTS). */
  ttsEnabled: boolean;
  /** Kokoro voice id (speaker index). */
  voiceSid: number;
  /** espeak-ng language for TTS phonemization (e.g. "pt-br", "en-us"). */
  voiceLang: string;
  // ── Voice (transient runtime state) ───────────────────────────────────────
  /** True while OxyVoice is capturing a spoken command. */
  listening: boolean;
  /** Live interim transcript shown in the listening overlay. */
  interim: string;
  setSessionId: (id: string | null) => void;
  toggle: () => void;
  setOpen: (open: boolean) => void;
  setWidth: (width: number) => void;
  setWakeEnabled: (on: boolean) => void;
  setKeyword: (k: string) => void;
  setThreshold: (t: number) => void;
  setDevice: (d: string) => void;
  setListening: (l: boolean) => void;
  setInterim: (s: string) => void;
  setTtsEnabled: (on: boolean) => void;
  setVoiceSid: (sid: number) => void;
  setVoiceLang: (lang: string) => void;
}

export const useOxyStore = create<OxyState>((set) => ({
  sessionId:
    typeof localStorage !== "undefined"
      ? localStorage.getItem(STORAGE_KEY)
      : null,
  open: false,
  width: initialWidth(),
  wakeEnabled: ls(WAKE_KEY, "0") === "1",
  keyword: ls(KEYWORD_KEY, "OXY"),
  threshold: Number(ls(THRESHOLD_KEY, "0.2")) || 0.2,
  device: ls(DEVICE_KEY, ""),
  ttsEnabled: ls(TTS_KEY, "0") === "1",
  // 42 = pf_dora, the pt-BR female voice in kokoro-multi-lang-v1_0.
  voiceSid: Number(ls(SID_KEY, "42")) || 42,
  voiceLang: ls(LANG_KEY, "pt-br"),
  listening: false,
  interim: "",
  setSessionId: (id) => {
    if (typeof localStorage !== "undefined") {
      if (id) localStorage.setItem(STORAGE_KEY, id);
      else localStorage.removeItem(STORAGE_KEY);
    }
    set({ sessionId: id });
  },
  toggle: () => set((s) => ({ open: !s.open })),
  setOpen: (open) => set({ open }),
  setWidth: (width) => {
    const clamped = Math.min(OXY_MAX_WIDTH, Math.max(OXY_MIN_WIDTH, width));
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(WIDTH_KEY, String(clamped));
    }
    set({ width: clamped });
  },
  setWakeEnabled: (on) => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(WAKE_KEY, on ? "1" : "0");
    }
    set({ wakeEnabled: on });
  },
  setKeyword: (k) => {
    if (typeof localStorage !== "undefined") localStorage.setItem(KEYWORD_KEY, k);
    set({ keyword: k });
  },
  setThreshold: (t) => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(THRESHOLD_KEY, String(t));
    }
    set({ threshold: t });
  },
  setDevice: (d) => {
    if (typeof localStorage !== "undefined") localStorage.setItem(DEVICE_KEY, d);
    set({ device: d });
  },
  setListening: (l) => set({ listening: l }),
  setInterim: (s) => set({ interim: s }),
  setTtsEnabled: (on) => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(TTS_KEY, on ? "1" : "0");
    }
    set({ ttsEnabled: on });
  },
  setVoiceSid: (sid) => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(SID_KEY, String(sid));
    }
    set({ voiceSid: sid });
  },
  setVoiceLang: (lang) => {
    if (typeof localStorage !== "undefined") localStorage.setItem(LANG_KEY, lang);
    set({ voiceLang: lang });
  },
}));
