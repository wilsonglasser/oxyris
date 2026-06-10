import { create } from "zustand";
import {
  actionKill,
  actionRun,
  listenActionOutput,
  type ActionKind,
  type ActionStreamLine,
} from "~/ipc/actions.ts";
import { useTerminalDockStore } from "~/stores/terminalDockStore.ts";

/**
 * In-memory store of every action run that's been started this session.
 * Each run keeps the rolling line buffer + status + Tauri unlisten so the
 * modal can be minimized + reopened without losing output.
 *
 * Keyed by `actionId` so the sidebar's badge counter is `runs[actionId].length`.
 */

export type RunLine =
  | { kind: "stdout"; text: string }
  | { kind: "stderr"; text: string };

export type RunStatus =
  | { kind: "running" }
  | { kind: "done"; code: number; success: boolean }
  | { kind: "error"; message: string };

export type RunInstance = {
  runId: string;
  actionId: string;
  actionName: string;
  startedAt: number;
  lines: RunLine[];
  status: RunStatus;
  unlisten: (() => void) | null;
};

interface State {
  runs: Record<string, RunInstance[]>;
  /** Action ids whose modal is currently open (visible). */
  openActionIds: Record<string, boolean>;
  /** Per-action active tab (runId) when there are 2+ instances. */
  activeTabRun: Record<string, string>;

  start: (
    action: {
      id: string;
      name: string;
      kind: ActionKind;
      command: string;
    },
    projectId: string,
    worktreeId: string | null,
    sessionId: string | null,
    onOpenTerminal: () => void,
  ) => Promise<void>;
  toggleOpen: (actionId: string) => void;
  setOpen: (actionId: string, open: boolean) => void;
  setActiveTab: (actionId: string, runId: string) => void;
  /** Drop a run completely (used by the modal's X button). */
  killRun: (actionId: string, runId: string) => void;
  /** Drop every finished run for an action (one-click "clean"). */
  pruneFinished: (actionId: string) => void;
}

export const useActionRunsStore = create<State>((set, get) => ({
  runs: {},
  openActionIds: {},
  activeTabRun: {},

  start: async (action, projectId, worktreeId, sessionId, onOpenTerminal) => {
    if (action.kind === "terminal_command_pty") {
      if (!sessionId) {
        throw new Error(
          "An active session is required to run an interactive terminal action.",
        );
      }
      // Hand off to the dock — it owns the PTY tabs, so it must be the one to
      // spawn (otherwise the new tab is invisible until an unrelated refresh).
      // Open the dock first so it mounts and drains this request.
      useTerminalDockStore.getState().enqueue(sessionId, action.command);
      onOpenTerminal();
      return;
    }
    const { run_id } = await actionRun({
      action_id: action.id,
      project_id: projectId,
      worktree_id: worktreeId,
    });
    const instance: RunInstance = {
      runId: run_id,
      actionId: action.id,
      actionName: action.name,
      startedAt: Date.now(),
      lines: [],
      status: { kind: "running" },
      unlisten: null,
    };
    set((s) => ({
      runs: {
        ...s.runs,
        [action.id]: [...(s.runs[action.id] ?? []), instance],
      },
      openActionIds: { ...s.openActionIds, [action.id]: true },
      activeTabRun: { ...s.activeTabRun, [action.id]: run_id },
    }));

    // Subscribe — keeps appending lines to this instance even when the
    // modal is minimized. Drops cleanly on `killRun`.
    const unlisten = await listenActionOutput(run_id, (line) => {
      mutateInstance(set, get, action.id, run_id, (inst) => {
        if (line.kind === "batch") {
          // Backend coalesces output into ~50ms batches so a chatty process
          // like `cargo run` triggers one store update per batch instead of
          // one per line (which froze the whole app).
          for (const chunk of line.lines) {
            inst.lines.push({ kind: chunk.stream, text: chunk.text });
          }
          // Cap at 5000 lines per instance so a chatty process doesn't
          // gobble RAM forever; oldest go first.
          if (inst.lines.length > 5000) {
            inst.lines.splice(0, inst.lines.length - 5000);
          }
        } else if (line.kind === "exit") {
          inst.status = { kind: "done", code: line.code, success: line.success };
        } else {
          inst.status = { kind: "error", message: line.message };
        }
      });
    });
    mutateInstance(set, get, action.id, run_id, (inst) => {
      inst.unlisten = unlisten;
    });
  },

  toggleOpen: (actionId) =>
    set((s) => {
      const open = !(s.openActionIds[actionId] ?? false);
      return { openActionIds: { ...s.openActionIds, [actionId]: open } };
    }),

  setOpen: (actionId, open) =>
    set((s) => ({
      openActionIds: { ...s.openActionIds, [actionId]: open },
    })),

  setActiveTab: (actionId, runId) =>
    set((s) => ({
      activeTabRun: { ...s.activeTabRun, [actionId]: runId },
    })),

  killRun: (actionId, runId) =>
    set((s) => {
      const list = (s.runs[actionId] ?? []).filter((r) => {
        if (r.runId === runId) {
          r.unlisten?.();
          // Tree-kill the OS process so a closed `watch` actually stops
          // instead of running on headless. No-op once it has exited.
          if (r.status.kind === "running") void actionKill(runId);
          return false;
        }
        return true;
      });
      const nextRuns = { ...s.runs, [actionId]: list };
      const stillActiveTab = list.find((r) => r.runId === s.activeTabRun[actionId]);
      const nextActive = { ...s.activeTabRun };
      if (!stillActiveTab && list.length > 0 && list[list.length - 1]) {
        nextActive[actionId] = list[list.length - 1]!.runId;
      } else if (list.length === 0) {
        delete nextActive[actionId];
      }
      // Auto-close the modal if no instances remain.
      const nextOpen = { ...s.openActionIds };
      if (list.length === 0) {
        delete nextOpen[actionId];
      }
      return {
        runs: nextRuns,
        activeTabRun: nextActive,
        openActionIds: nextOpen,
      };
    }),

  pruneFinished: (actionId) =>
    set((s) => {
      const list = (s.runs[actionId] ?? []).filter((r) => {
        if (r.status.kind === "running") return true;
        r.unlisten?.();
        return false;
      });
      return { runs: { ...s.runs, [actionId]: list } };
    }),
}));

function mutateInstance(
  set: (updater: (s: State) => Partial<State>) => void,
  get: () => State,
  actionId: string,
  runId: string,
  fn: (inst: RunInstance) => void,
) {
  const list = get().runs[actionId];
  if (!list) return;
  const idx = list.findIndex((r) => r.runId === runId);
  if (idx === -1) return;
  const copy = [...list];
  const next: RunInstance = { ...copy[idx]!, lines: [...copy[idx]!.lines] };
  fn(next);
  copy[idx] = next;
  set((s) => ({ runs: { ...s.runs, [actionId]: copy } }));
  // Suppress unused-arg lint (set already captures everything we need).
  void runId;
}

/** Convenience event subscriber for the streaming line type. */
export type { ActionStreamLine };
