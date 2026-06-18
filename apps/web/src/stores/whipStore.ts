import { create } from "zustand";

/**
 * "Whip mode" — a playful effort-booster modeled on OpenWhip. When armed
 * (Ctrl+W or the header button), the cursor turns into a whip and clicking a
 * pure-mode Claude terminal injects an escalating thinking keyword into its TUI
 * prompt + Enter, bumping the reasoning effort of the next message.
 *
 * The ladder maps each "crack" to a stronger thinking keyword the `claude` CLI
 * recognizes; once at the top (`ultrathink`) further cracks stay pinned there.
 * Esc disarms (see App's global keydown). State is in-memory only — whip mode is
 * a transient gesture, not a persisted preference.
 */
export const WHIP_LADDER = [
  "think",
  "think hard",
  "think harder",
  "ultrathink",
] as const;

interface WhipState {
  /** Whip cursor armed globally. Toggled by Ctrl+W / the header button. */
  active: boolean;
  /** Per-session rung index into {@link WHIP_LADDER}. */
  rung: Record<string, number>;
  setActive: (on: boolean) => void;
  toggle: () => void;
  /**
   * Crack the whip at a session: escalate its rung one step (capped at the top)
   * and return the thinking keyword to inject. Leading space included so it
   * doesn't glue onto whatever the user already typed in the TUI prompt.
   */
  crack: (sessionId: string) => string;
}

export const useWhipStore = create<WhipState>((set, get) => ({
  active: false,
  rung: {},
  setActive: (on) => set({ active: on }),
  toggle: () => set((s) => ({ active: !s.active })),
  crack: (sessionId) => {
    const cur = get().rung[sessionId] ?? -1;
    const next = Math.min(cur + 1, WHIP_LADDER.length - 1);
    set((s) => ({ rung: { ...s.rung, [sessionId]: next } }));
    return ` ${WHIP_LADDER[next]}`;
  },
}));
