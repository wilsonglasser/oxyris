import { create } from "zustand";
import {
  actionRun,
  listenActionOutput,
  type ActionStreamLine,
} from "~/ipc/actions.ts";

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
    actionId: string,
    actionName: string,
    projectId: string,
    worktreeId: string | null,
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

  start: async (actionId, actionName, projectId, worktreeId) => {
    const { run_id } = await actionRun({
      action_id: actionId,
      project_id: projectId,
      worktree_id: worktreeId,
    });
    const instance: RunInstance = {
      runId: run_id,
      actionId,
      actionName,
      startedAt: Date.now(),
      lines: [],
      status: { kind: "running" },
      unlisten: null,
    };
    set((s) => ({
      runs: {
        ...s.runs,
        [actionId]: [...(s.runs[actionId] ?? []), instance],
      },
      openActionIds: { ...s.openActionIds, [actionId]: true },
      activeTabRun: { ...s.activeTabRun, [actionId]: run_id },
    }));

    // Subscribe — keeps appending lines to this instance even when the
    // modal is minimized. Drops cleanly on `killRun`.
    const unlisten = await listenActionOutput(run_id, (line) => {
      mutateInstance(set, get, actionId, run_id, (inst) => {
        if (line.kind === "stdout" || line.kind === "stderr") {
          inst.lines.push(line);
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
    mutateInstance(set, get, actionId, run_id, (inst) => {
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
