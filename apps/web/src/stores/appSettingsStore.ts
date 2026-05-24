import { create } from "zustand";

/**
 * App-wide UI preferences that don't belong to any one session. Persisted to
 * localStorage so the choice survives reloads. Kept deliberately tiny — this
 * is not the place for per-project or per-session config.
 */

const PURE_MODE_KEY = "oxyris.pureMode";
const OPEN_EXTERNAL_KEY = "oxyris.openFilesExternally";

function loadPureMode(): boolean {
  return window.localStorage.getItem(PURE_MODE_KEY) === "1";
}

function loadOpenExternal(): boolean {
  return window.localStorage.getItem(OPEN_EXTERNAL_KEY) === "1";
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
}));
