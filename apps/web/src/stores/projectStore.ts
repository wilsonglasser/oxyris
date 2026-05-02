import { create } from "zustand";
import { type ProjectRow, projectList } from "~/ipc/commands.ts";

interface ProjectStoreState {
  projects: ProjectRow[];
  activeId: string | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  setActive: (id: string | null) => void;
  /** Lookup the full row by id; `null` if not found. */
  active: () => ProjectRow | null;
}

export const useProjectStore = create<ProjectStoreState>((set, get) => ({
  projects: [],
  activeId: null,
  loading: false,
  error: null,

  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const rows = await projectList();
      const { activeId } = get();
      const stillExists = rows.some((r) => r.id === activeId);
      set({
        projects: rows,
        activeId: stillExists
          ? activeId
          : rows.length > 0
            ? rows[0]!.id
            : null,
        loading: false,
      });
    } catch (err) {
      set({
        loading: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  setActive: (id) => set({ activeId: id }),

  active: () => {
    const { projects, activeId } = get();
    if (!activeId) return null;
    return projects.find((p) => p.id === activeId) ?? null;
  },
}));
