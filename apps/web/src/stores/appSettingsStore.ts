import { create } from "zustand";
import {
  type ClaudeLanguage,
  isClaudeLanguage,
} from "~/lib/claudeLanguage.ts";

/**
 * App-wide UI preferences that don't belong to any one session. Persisted to
 * localStorage so the choice survives reloads. Kept deliberately tiny — this
 * is not the place for per-project or per-session config.
 */

const PURE_MODE_KEY = "oxyris.pureMode";
const OPEN_EXTERNAL_KEY = "oxyris.openFilesExternally";
const TERM_FONT_KEY = "oxyris.terminalFontSize";
const CLAUDE_LANG_KEY = "oxyris.claudeLanguage";

/** Default xterm font size (px). Base 100% for the zoom indicator. */
export const TERM_FONT_DEFAULT = 12;
const TERM_FONT_MIN = 6;
const TERM_FONT_MAX = 40;

function clampFont(px: number): number {
  if (!Number.isFinite(px)) return TERM_FONT_DEFAULT;
  return Math.min(TERM_FONT_MAX, Math.max(TERM_FONT_MIN, Math.round(px)));
}

function loadPureMode(): boolean {
  return window.localStorage.getItem(PURE_MODE_KEY) === "1";
}

function loadOpenExternal(): boolean {
  return window.localStorage.getItem(OPEN_EXTERNAL_KEY) === "1";
}

function loadTerminalFontSize(): number {
  const raw = window.localStorage.getItem(TERM_FONT_KEY);
  return raw ? clampFont(Number(raw)) : TERM_FONT_DEFAULT;
}

function loadClaudeLanguage(): ClaudeLanguage {
  const raw = window.localStorage.getItem(CLAUDE_LANG_KEY);
  return isClaudeLanguage(raw) ? raw : "auto";
}

interface AppSettingsState {
  /**
   * When true, the chat surface is replaced by the "Claude Code puro" panel:
   * the interactive `claude` TUI running in a PTY instead of our structured
   * event-sourced chat. Global toggle, not per-thread.
   */
  pureMode: boolean;
  setPureMode: (on: boolean) => void;
  /**
   * Ctrl/Cmd+clicking a file path in a terminal opens it. When false (default)
   * the file opens in an in-app modal editor; when true it's handed to the
   * user's external editor via `fs_open_external`.
   */
  openFilesExternally: boolean;
  setOpenFilesExternally: (on: boolean) => void;
  /**
   * Font size (px) shared by every xterm terminal. Adjusted live via Ctrl +/-,
   * Ctrl+0 (reset) and Ctrl+scroll. Global, not per-terminal, so all panes zoom
   * together; persisted so the choice survives reloads.
   */
  terminalFontSize: number;
  setTerminalFontSize: (px: number) => void;
  /** Step the font size by `delta` px, clamped to the allowed range. */
  bumpTerminalFontSize: (delta: number) => void;
  resetTerminalFontSize: () => void;
  /**
   * Language Claude is instructed to reply in, independent of the UI locale.
   * `"auto"` (default) tells Claude to mirror the user's own language instead
   * of drifting to the (mostly English) code/context. Fed into every session's
   * system prompt at start — see {@link import("~/lib/claudeLanguage").claudeLanguageDirective}.
   */
  claudeLanguage: ClaudeLanguage;
  setClaudeLanguage: (lang: ClaudeLanguage) => void;
}

export const useAppSettingsStore = create<AppSettingsState>((set) => ({
  pureMode: loadPureMode(),
  setPureMode: (on) => {
    window.localStorage.setItem(PURE_MODE_KEY, on ? "1" : "0");
    set({ pureMode: on });
  },
  openFilesExternally: loadOpenExternal(),
  setOpenFilesExternally: (on) => {
    window.localStorage.setItem(OPEN_EXTERNAL_KEY, on ? "1" : "0");
    set({ openFilesExternally: on });
  },
  terminalFontSize: loadTerminalFontSize(),
  setTerminalFontSize: (px) => {
    const next = clampFont(px);
    window.localStorage.setItem(TERM_FONT_KEY, String(next));
    set({ terminalFontSize: next });
  },
  bumpTerminalFontSize: (delta) =>
    set((s) => {
      const next = clampFont(s.terminalFontSize + delta);
      window.localStorage.setItem(TERM_FONT_KEY, String(next));
      return { terminalFontSize: next };
    }),
  resetTerminalFontSize: () => {
    window.localStorage.setItem(TERM_FONT_KEY, String(TERM_FONT_DEFAULT));
    set({ terminalFontSize: TERM_FONT_DEFAULT });
  },
  claudeLanguage: loadClaudeLanguage(),
  setClaudeLanguage: (lang) => {
    window.localStorage.setItem(CLAUDE_LANG_KEY, lang);
    set({ claudeLanguage: lang });
  },
}));
