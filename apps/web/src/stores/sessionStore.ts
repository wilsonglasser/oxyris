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
  setActive: (id: string | null) => void;
  hydrate: (snapshot: SessionSnapshot) => void;
  applyEvent: (ev: EmittedSessionEvent) => void;
  clear: (id: string) => void;
  drop: (id: string) => void;
}

export const useSessionStore = create<SessionStoreState>((set) => ({
  snapshots: {},
  activeSessionId: null,

  setActive: (id) => set({ activeSessionId: id }),

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
      return {
        snapshots: rest,
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
