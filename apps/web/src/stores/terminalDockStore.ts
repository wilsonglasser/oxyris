import { create } from "zustand";

/**
 * Bridge between code that wants a command run in an interactive terminal
 * (PTY actions, auto-run-on-worktree-create) and the dock that actually owns
 * the PTY tabs (`TerminalPanel`).
 *
 * The dock keeps its tab list in local React state, so spawning a PTY directly
 * via the IPC layer would leave the dock unaware of it — the tab wouldn't show
 * until an unrelated refresh (e.g. switching projects). Instead, callers
 * `enqueue` a request here; the mounted dock for that session drains the queue
 * with `take`, spawns the tab itself (updating its own state), and writes the
 * command. Requests sit in the queue until a dock for that session mounts, so
 * the order is: enqueue → open the dock → dock consumes.
 */
export interface TerminalSpawnRequest {
  reqId: number;
  sessionId: string;
  command: string;
}

interface TerminalDockState {
  requests: TerminalSpawnRequest[];
  nextId: number;
  /** Queue a command to run in a fresh terminal tab for `sessionId`. */
  enqueue: (sessionId: string, command: string) => void;
  /** Pull (and remove) every pending request for `sessionId`. */
  take: (sessionId: string) => TerminalSpawnRequest[];
}

export const useTerminalDockStore = create<TerminalDockState>((set, get) => ({
  requests: [],
  nextId: 1,
  enqueue: (sessionId, command) =>
    set((s) => ({
      requests: [...s.requests, { reqId: s.nextId, sessionId, command }],
      nextId: s.nextId + 1,
    })),
  take: (sessionId) => {
    const mine = get().requests.filter((r) => r.sessionId === sessionId);
    if (mine.length > 0) {
      set((s) => ({
        requests: s.requests.filter((r) => r.sessionId !== sessionId),
      }));
    }
    return mine;
  },
}));
