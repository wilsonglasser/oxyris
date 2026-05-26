import { create } from "zustand";
import type {
  AssistantBlock,
  EmittedSessionEvent,
  SessionSnapshot,
  TurnEntry,
} from "~/ipc/session.ts";

interface SessionStoreState {
  snapshots: Record<string, SessionSnapshot>;
  activeSessionId: string | null;
  /**
   * Background threads (not the active one) whose turn finished or errored —
   * flagged so the sidebar can highlight them until the user opens them.
   */
  attention: Record<string, boolean>;
  /**
   * Threads paused on a tool-approval prompt — Claude wants the user to pick
   * an input. Surfaced as the red bull. Unlike {@link attention}, this is set
   * for the active thread too (the dot must read red while you decide) and is
   * *not* cleared by opening the thread — only by answering or the turn ending.
   */
  needsInput: Record<string, boolean>;
  setActive: (id: string | null) => void;
  /** Flag a background thread as needing attention (no-op for the active one). */
  markAttention: (id: string) => void;
  /** Set/clear the "Claude wants your input" (red) flag for a thread. */
  setNeedsInput: (id: string, on: boolean) => void;
  hydrate: (snapshot: SessionSnapshot) => void;
  applyEvent: (ev: EmittedSessionEvent) => void;
  clear: (id: string) => void;
  drop: (id: string) => void;
}

export const useSessionStore = create<SessionStoreState>((set) => ({
  snapshots: {},
  activeSessionId: null,
  attention: {},
  needsInput: {},

  setActive: (id) =>
    set((state) => {
      // Opening a thread clears its attention flag.
      if (id && state.attention[id]) {
        const { [id]: _seen, ...rest } = state.attention;
        return { activeSessionId: id, attention: rest };
      }
      return { activeSessionId: id };
    }),

  markAttention: (id) =>
    set((state) =>
      // Never flag the thread the user is already looking at, and skip a
      // redundant write if it's already flagged.
      id === state.activeSessionId || state.attention[id]
        ? {}
        : { attention: { ...state.attention, [id]: true } },
    ),

  setNeedsInput: (id, on) =>
    set((state) => {
      if (!!state.needsInput[id] === on) return {};
      if (on) return { needsInput: { ...state.needsInput, [id]: true } };
      const { [id]: _drop, ...rest } = state.needsInput;
      return { needsInput: rest };
    }),

  hydrate: (snapshot) =>
    set((state) => ({
      snapshots: { ...state.snapshots, [snapshot.id]: snapshot },
    })),

  clear: (id) =>
    set((state) => {
      const { [id]: _removed, ...rest } = state.snapshots;
      return { snapshots: rest };
    }),

  drop: (id) =>
    set((state) => {
      const { [id]: _removed, ...rest } = state.snapshots;
      const { [id]: _seen, ...attRest } = state.attention;
      const { [id]: _input, ...inputRest } = state.needsInput;
      return {
        snapshots: rest,
        attention: attRest,
        needsInput: inputRest,
        activeSessionId: state.activeSessionId === id ? null : state.activeSessionId,
      };
    }),

  applyEvent: ({ session_id, event }) =>
    set((state) => {
      const current = state.snapshots[session_id];
      const next = applyEventToSnapshot(current, event, session_id);
      return {
        snapshots: { ...state.snapshots, [session_id]: next },
      };
    }),
}));

function applyEventToSnapshot(
  current: SessionSnapshot | undefined,
  event: EmittedSessionEvent["event"],
  sessionId: string,
): SessionSnapshot {
  if (event.kind === "SessionStarted") {
    return {
      id: event.id,
      project_id: event.project_id,
      worktree_id: event.worktree_id,
      provider_id: event.provider_id,
      model: event.model,
      thinking: event.thinking,
      runtime: event.runtime,
      status: "running",
      turns: [],
      created_at: event.created_at,
      provider_session_id: null,
      title: null,
      env_mode: event.env_mode,
      kind: event.session_kind,
      pinned_at: null,
    };
  }
  if (!current) {
    // Event arrived before we hydrated — skeleton it so we don't drop state.
    return {
      id: sessionId,
      project_id: "",
      worktree_id: null,
      provider_id: "",
      model: "",
      thinking: "auto",
      runtime: "supervised",
      status: "running",
      turns: [],
      created_at: new Date().toISOString(),
      provider_session_id: null,
      title: null,
      env_mode: "default",
      kind: "structured",
      pinned_at: null,
    };
  }
  switch (event.kind) {
    case "SessionStopped":
      return { ...current, status: "stopped" };
    case "SessionErrored":
      return { ...current, status: "errored" };
    case "TurnStarted": {
      const turn: TurnEntry = {
        id: event.turn_id,
        user_text: event.user_text,
        blocks: [],
        status: "streaming",
        started_at: event.started_at,
        completed_at: null,
        total_cost_usd: null,
        input_tokens: null,
        output_tokens: null,
        error_message: null,
      };
      return { ...current, turns: [...current.turns, turn] };
    }
    case "TurnAssistantBlockAppended":
      return {
        ...current,
        turns: current.turns.map((t) =>
          t.id === event.turn_id
            ? { ...t, blocks: mergeBlock(t.blocks, event.block) }
            : t,
        ),
      };
    case "TurnCompleted":
      return {
        ...current,
        turns: current.turns.map((t) =>
          t.id === event.turn_id
            ? {
                ...t,
                status: "completed",
                completed_at: event.completed_at,
                total_cost_usd: event.total_cost_usd,
                input_tokens: event.input_tokens,
                output_tokens: event.output_tokens,
              }
            : t,
        ),
      };
    case "TurnFailed":
      return {
        ...current,
        turns: current.turns.map((t) =>
          t.id === event.turn_id
            ? {
                ...t,
                status: "failed",
                completed_at: event.completed_at,
                error_message: event.message,
              }
            : t,
        ),
      };
    case "TurnInterrupted":
      return {
        ...current,
        turns: current.turns.map((t) =>
          t.id === event.turn_id
            ? { ...t, status: "interrupted", completed_at: event.at }
            : t,
        ),
      };
    case "ProviderSessionAttached":
      return { ...current, provider_session_id: event.provider_session_id };
    case "SessionResumed":
      return { ...current, status: "running" };
    case "SessionRenamed":
      return { ...current, title: event.title };
    case "SessionPinToggled":
      return { ...current, pinned_at: event.pinned_at };
    case "SessionEnvModeChanged":
      return { ...current, env_mode: event.mode };
    case "SessionDeleted":
      // Caller is expected to also drop the snapshot via `clear(id)`; we
      // keep the shape valid here as a defensive fallback.
      return current;
  }
}

/**
 * Mirror of the backend's block merger: consecutive text blocks coalesce so
 * the thread shows one bubble per assistant text burst instead of dozens of
 * tiny appends.
 */
function mergeBlock(
  blocks: AssistantBlock[],
  incoming: AssistantBlock,
): AssistantBlock[] {
  const last = blocks[blocks.length - 1];
  if (last && last.kind === "text" && incoming.kind === "text") {
    return [
      ...blocks.slice(0, -1),
      { kind: "text", text: last.text + incoming.text },
    ];
  }
  return [...blocks, incoming];
}
