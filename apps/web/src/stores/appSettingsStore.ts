import { create } from "zustand";

/**
 * App-wide UI preferences that don't belong to any one session. Persisted to
 * localStorage so the choice survives reloads. Kept deliberately tiny — this
 * is not the place for per-project or per-session config.
 */

const PURE_MODE_KEY = "oxyris.pureMode";

function loadPureMode(): boolean {
  return window.localStorage.getItem(PURE_MODE_KEY) === "1";
}

interface AppSettingsState {
  /**
   * When true, the chat surface is replaced by the "Claude Code puro" panel:
   * the interactive `claude` TUI running in a PTY instead of our structured
   * event-sourced chat. Global toggle, not per-thread.
   */
  pureMode: boolean;
  setPureMode: (on: boolean) => void;
}

export const useAppSettingsStore = create<AppSettingsState>((set) => ({
  pureMode: loadPureMode(),
  setPureMode: (on) => {
    window.localStorage.setItem(PURE_MODE_KEY, on ? "1" : "0");
    set({ pureMode: on });
  },
}));
