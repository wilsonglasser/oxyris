import { create } from "zustand";
import {
  type ActionKind,
  type ActionRow,
  actionDelete,
  actionList,
  actionUpsert,
} from "~/ipc/actions.ts";

interface State {
  byProject: Record<string, ActionRow[]>;
  loading: Record<string, boolean>;
  refresh: (projectId: string) => Promise<void>;
  upsert: (input: {
    id?: string | null;
    project_id: string;
    name: string;
    command: string;
    keybinding?: string | null;
    auto_run_on_worktree_create: boolean;
    icon: string;
    kind: ActionKind;
    show_in_sidebar?: boolean;
  }) => Promise<ActionRow>;
  remove: (projectId: string, id: string) => Promise<void>;
}

const EMPTY_ACTIONS: ActionRow[] = [];

export const useActionsStore = create<State>((set, get) => ({
  byProject: {},
  loading: {},

  refresh: async (projectId) => {
    set((s) => ({ loading: { ...s.loading, [projectId]: true } }));
    try {
      const rows = await actionList({ project_id: projectId });
      set((s) => ({
        byProject: { ...s.byProject, [projectId]: rows },
      }));
    } finally {
      set((s) => ({ loading: { ...s.loading, [projectId]: false } }));
    }
  },

  upsert: async (input) => {
    const row = await actionUpsert(input);
    set((s) => {
      const prev = s.byProject[input.project_id] ?? [];
      const next =
        prev.findIndex((a) => a.id === row.id) === -1
          ? [...prev, row]
          : prev.map((a) => (a.id === row.id ? row : a));
      return { byProject: { ...s.byProject, [input.project_id]: next } };
    });
    return row;
  },

  remove: async (projectId, id) => {
    await actionDelete({ id });
    set((s) => {
      const prev = s.byProject[projectId] ?? [];
      return {
        byProject: {
          ...s.byProject,
          [projectId]: prev.filter((a) => a.id !== id),
        },
      };
    });
    // Reconcile with backend so soft-deleted rows are dropped.
    void get().refresh(projectId);
  },
}));

export function useProjectActions(projectId: string): ActionRow[] {
  return useActionsStore((s) => s.byProject[projectId]) ?? EMPTY_ACTIONS;
}
