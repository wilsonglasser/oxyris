import { create } from "zustand";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Mirrors `infra::lsp::LspStatusEvent` on the backend. Streamed via the
 * `lsp:status` Tauri event whenever a language server transitions in our
 * pool (spawning → ready, or → failed / not_installed).
 */
export type LspStatusEvent =
  | { phase: "spawning"; worktree_id: string; language: string }
  | { phase: "ready"; worktree_id: string; language: string }
  | { phase: "failed"; worktree_id: string; language: string; error: string }
  | {
      phase: "not_installed";
      worktree_id: string;
      language: string;
      hint: string;
    };

export interface LanguageState {
  language: string;
  phase: "spawning" | "ready" | "failed" | "not_installed";
  message: string | null;
  updatedAt: number;
}

interface LspStatusStore {
  /** worktree_id → language → status */
  byWorktree: Record<string, Record<string, LanguageState>>;
  apply: (e: LspStatusEvent) => void;
  clear: (worktreeId: string) => void;
  subscribe: () => Promise<UnlistenFn>;
}

export const useLspStatusStore = create<LspStatusStore>((set) => ({
  byWorktree: {},
  apply: (e) =>
    set((state) => {
      const wt = state.byWorktree[e.worktree_id] ?? {};
      const next: LanguageState = {
        language: e.language,
        phase: e.phase,
        message:
          e.phase === "failed"
            ? e.error
            : e.phase === "not_installed"
              ? e.hint
              : null,
        updatedAt: Date.now(),
      };
      return {
        byWorktree: {
          ...state.byWorktree,
          [e.worktree_id]: { ...wt, [e.language]: next },
        },
      };
    }),
  clear: (worktreeId) =>
    set((state) => {
      const { [worktreeId]: _drop, ...rest } = state.byWorktree;
      return { byWorktree: rest };
    }),
  subscribe: async () =>
    listen<LspStatusEvent>("lsp:status", (event) => {
      useLspStatusStore.getState().apply(event.payload);
    }),
}));
