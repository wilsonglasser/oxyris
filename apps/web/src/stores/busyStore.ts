import { create } from "zustand";

/**
 * Per-session "a turn is in flight" flag, surfaced as a pulsing dot in the
 * sidebar. Distinct from `sessionStore.attention` (which flags a *finished*
 * background thread) and from the session-list `status` (session-level
 * running, not turn-level activity).
 *
 * Fed from two places, since the two session kinds expose progress differently:
 * - Structured threads — TurnStarted → true, terminal outcome → false. The
 *   active thread is driven by ChatPanel; background threads by the Sidebar
 *   watcher that already listens to every running session.
 * - Pure threads — no turn-event stream, so PureClaudePanel mirrors its own
 *   armed/idle heuristic here.
 */
interface BusyState {
  busy: Record<string, boolean>;
  setBusy: (sessionId: string, on: boolean) => void;
  /** Forget a session entirely (on delete). Without this a session deleted
   * while busy leaks `busy[id]=true` forever — the sidebar dot (and the
   * project busy-tier roll-up) stays blue indefinitely. */
  drop: (sessionId: string) => void;
}

export const useBusyStore = create<BusyState>((set) => ({
  busy: {},
  setBusy: (sessionId, on) =>
    set((s) => {
      if (!!s.busy[sessionId] === on) return s;
      if (on) return { busy: { ...s.busy, [sessionId]: true } };
      const { [sessionId]: _drop, ...rest } = s.busy;
      return { busy: rest };
    }),
  drop: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.busy)) return s;
      const { [sessionId]: _drop, ...rest } = s.busy;
      return { busy: rest };
    }),
}));
