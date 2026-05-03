import { create } from "zustand";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Mirrors `infra::indexing::IndexingProgress` on the backend. Streamed via
 * the `indexing:progress` Tauri event during initial worktree walks.
 */
export type IndexingProgress =
  | {
      phase: "started";
      worktree_id: string;
      total_files: number;
    }
  | {
      phase: "progress";
      worktree_id: string;
      files_indexed: number;
      files_skipped: number;
    }
  | {
      phase: "done";
      worktree_id: string;
      report: {
        files_indexed: number;
        symbols_extracted: number;
        files_skipped: number;
        bytes_read: number;
        duration_ms: number;
      };
    }
  | {
      phase: "failed";
      worktree_id: string;
      error: string;
    };

export interface WorktreeProgress {
  /** Latest phase the backend reported. */
  phase: "started" | "progress" | "done" | "failed";
  /** Files matched by the cheap pre-walk (set on `started`). */
  totalFiles: number;
  /** Files actually indexed so far (live counter). */
  filesIndexed: number;
  /** Files we skipped (too big, unreadable, parse error, etc.). */
  filesSkipped: number;
  /** Final summary, populated on `done`. */
  symbols: number | null;
  durationMs: number | null;
  /** Error message when phase = "failed". */
  error: string | null;
  /** Wall-clock the last update arrived — used to auto-hide stale "done"s. */
  updatedAt: number;
}

interface IndexingStore {
  byWorktree: Record<string, WorktreeProgress>;
  apply: (p: IndexingProgress) => void;
  clear: (worktreeId: string) => void;
  /** Wires the global Tauri listener once. Returns unsubscribe. */
  subscribe: () => Promise<UnlistenFn>;
}

const empty: WorktreeProgress = {
  phase: "started",
  totalFiles: 0,
  filesIndexed: 0,
  filesSkipped: 0,
  symbols: null,
  durationMs: null,
  error: null,
  updatedAt: 0,
};

export const useIndexingStore = create<IndexingStore>((set) => ({
  byWorktree: {},
  apply: (p) =>
    set((state) => {
      const prev = state.byWorktree[p.worktree_id] ?? empty;
      const now = Date.now();
      let next: WorktreeProgress;
      if (p.phase === "started") {
        next = {
          ...empty,
          phase: "started",
          totalFiles: p.total_files,
          updatedAt: now,
        };
      } else if (p.phase === "progress") {
        next = {
          ...prev,
          phase: "progress",
          filesIndexed: p.files_indexed,
          filesSkipped: p.files_skipped,
          updatedAt: now,
        };
      } else if (p.phase === "done") {
        next = {
          ...prev,
          phase: "done",
          filesIndexed: p.report.files_indexed,
          filesSkipped: p.report.files_skipped,
          symbols: p.report.symbols_extracted,
          durationMs: p.report.duration_ms,
          updatedAt: now,
        };
      } else {
        next = {
          ...prev,
          phase: "failed",
          error: p.error,
          updatedAt: now,
        };
      }
      return {
        byWorktree: { ...state.byWorktree, [p.worktree_id]: next },
      };
    }),
  clear: (worktreeId) =>
    set((state) => {
      const { [worktreeId]: _drop, ...rest } = state.byWorktree;
      return { byWorktree: rest };
    }),
  subscribe: async () => {
    return listen<IndexingProgress>("indexing:progress", (event) => {
      useIndexingStore.getState().apply(event.payload);
    });
  },
}));
