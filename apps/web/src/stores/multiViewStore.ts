import { create } from "zustand";

/**
 * Multi View pane layout. A pane is a slot in the grid bound to one session
 * (or empty until the user picks one). Persisted to localStorage so the
 * arrangement survives reloads. Pane → session mapping only; the panes
 * themselves render the existing Chat/Pure panels.
 */

const PANES_KEY = "oxyris.multiView.panes";
const COLS_KEY = "oxyris.multiView.cols";
const SIDEBAR_HIDDEN_KEY = "oxyris.multiView.sidebarHidden";
/** Rows are capped at 3, so max panes = cols * 3 (6 / 9 / 12 / 15). */
const MAX_ROWS = 3;

export type MvCols = 2 | 3 | 4 | 5;

export interface Pane {
  paneId: string;
  sessionId: string | null;
}

function newPaneId(): string {
  return crypto.randomUUID?.() ?? `pane-${Date.now()}-${Math.random()}`;
}

export function maxPanes(cols: MvCols): number {
  return cols * MAX_ROWS;
}

function loadCols(): MvCols {
  const raw = Number(window.localStorage.getItem(COLS_KEY));
  return raw === 2 || raw === 4 || raw === 5 ? raw : 3;
}

function loadSidebarHidden(): boolean {
  return window.localStorage.getItem(SIDEBAR_HIDDEN_KEY) === "1";
}

function loadPanes(): Pane[] {
  try {
    const raw = window.localStorage.getItem(PANES_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Pane[];
      if (Array.isArray(parsed) && parsed.length > 0) return parsed;
    }
  } catch {
    /* fall through to default */
  }
  return [{ paneId: newPaneId(), sessionId: null }];
}

interface MultiViewState {
  panes: Pane[];
  cols: MvCols;
  /** Collapse the app sidebar to hand the grid the full width. */
  sidebarHidden: boolean;
  addPane: () => void;
  removePane: (paneId: string) => void;
  setPaneSession: (paneId: string, sessionId: string | null) => void;
  setCols: (cols: MvCols) => void;
  toggleSidebar: () => void;
  /** Replace all panes (used by autofill). */
  setPanes: (sessionIds: string[]) => void;
  /** Reorder: pull `fromPaneId` out and drop it at `toPaneId`'s slot. */
  movePane: (fromPaneId: string, toPaneId: string) => void;
}

function persistPanes(panes: Pane[]): Pane[] {
  window.localStorage.setItem(PANES_KEY, JSON.stringify(panes));
  return panes;
}

export const useMultiViewStore = create<MultiViewState>((set) => ({
  panes: loadPanes(),
  cols: loadCols(),
  sidebarHidden: loadSidebarHidden(),
  addPane: () =>
    set((s) =>
      s.panes.length >= maxPanes(s.cols)
        ? s
        : { panes: persistPanes([...s.panes, { paneId: newPaneId(), sessionId: null }]) },
    ),
  removePane: (paneId) =>
    set((s) => {
      const next = s.panes.filter((p) => p.paneId !== paneId);
      // Always keep at least one pane so the grid never goes empty.
      return {
        panes: persistPanes(
          next.length > 0 ? next : [{ paneId: newPaneId(), sessionId: null }],
        ),
      };
    }),
  setPaneSession: (paneId, sessionId) =>
    set((s) => ({
      panes: persistPanes(
        s.panes.map((p) => (p.paneId === paneId ? { ...p, sessionId } : p)),
      ),
    })),
  setCols: (cols) => {
    window.localStorage.setItem(COLS_KEY, String(cols));
    set({ cols });
  },
  toggleSidebar: () =>
    set((s) => {
      const next = !s.sidebarHidden;
      window.localStorage.setItem(SIDEBAR_HIDDEN_KEY, next ? "1" : "0");
      return { sidebarHidden: next };
    }),
  setPanes: (sessionIds) =>
    set((s) => {
      const capped = sessionIds.slice(0, maxPanes(s.cols));
      const panes: Pane[] =
        capped.length > 0
          ? capped.map((sid) => ({ paneId: newPaneId(), sessionId: sid }))
          : [{ paneId: newPaneId(), sessionId: null }];
      return { panes: persistPanes(panes) };
    }),
  movePane: (fromPaneId, toPaneId) =>
    set((s) => {
      if (fromPaneId === toPaneId) return s;
      const from = s.panes.findIndex((p) => p.paneId === fromPaneId);
      const to = s.panes.findIndex((p) => p.paneId === toPaneId);
      if (from < 0 || to < 0) return s;
      const next = [...s.panes];
      const [moved] = next.splice(from, 1);
      next.splice(to, 0, moved!);
      return { panes: persistPanes(next) };
    }),
}));

/** Tailwind grid-cols class for the chosen column count (static for JIT). */
export function gridColsClass(cols: MvCols): string {
  switch (cols) {
    case 2:
      return "grid-cols-2";
    case 4:
      return "grid-cols-4";
    case 5:
      return "grid-cols-5";
    default:
      return "grid-cols-3";
  }
}
