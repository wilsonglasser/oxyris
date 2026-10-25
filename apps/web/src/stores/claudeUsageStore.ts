import { create } from "zustand";

import { onUsageBoard, type UsageBoard, usageBoard } from "~/ipc/claudeAccounts.ts";

/**
 * Mirror of the backend usage board. The backend owns polling and pushes
 * `claude-usage://board`; this store only listens and lets command results
 * (switch, save, remove) replace the board immediately.
 */
interface State {
  board: UsageBoard | null;
  setBoard: (board: UsageBoard) => void;
  /** Subscribe once and load the cached board. Safe to call repeatedly. */
  init: () => void;
}

let subscribed = false;

export const useClaudeUsageStore = create<State>((set) => ({
  board: null,
  setBoard: (board) => set({ board }),
  init: () => {
    if (subscribed) return;
    subscribed = true;
    void onUsageBoard((board) => set({ board }));
    void usageBoard()
      .then((board) => set({ board }))
      .catch(() => {});
  },
}));
