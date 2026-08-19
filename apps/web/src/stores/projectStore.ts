import { create } from "zustand";
import { type ProjectRow, projectList } from "~/ipc/commands.ts";

/** Sentinel for "show every workspace" in the sidebar selector. */
export const ALL_WORKSPACES = "__all__";
const WORKSPACE_FILTER_KEY = "oxyris.workspaceFilter";
const SIDEBAR_VIEW_KEY = "oxyris.sidebarView";

/**
 * How the sidebar lays out threads.
 *
 * - `tree` — the classic project → thread hierarchy (folders).
 * - `topics` — one flat list of threads across every visible project, pinned
 *   ones first, then most-recent. No folders, so each row carries the project
 *   badge to say which project it belongs to.
 */
export type SidebarView = "tree" | "topics";

function loadWorkspaceFilter(): string {
  try {
    return window.localStorage.getItem(WORKSPACE_FILTER_KEY) ?? ALL_WORKSPACES;
  } catch {
    return ALL_WORKSPACES;
  }
}

function loadSidebarView(): SidebarView {
  try {
    return window.localStorage.getItem(SIDEBAR_VIEW_KEY) === "topics"
      ? "topics"
      : "tree";
  } catch {
    return "tree";
  }
}

interface ProjectStoreState {
  projects: ProjectRow[];
  activeId: string | null;
  loading: boolean;
  error: string | null;
  /**
   * Sidebar workspace filter. `ALL_WORKSPACES` shows everything; any other
   * value shows only projects whose `workspace` matches (the empty string
   * matches ungrouped projects). Persisted across reloads.
   */
  workspaceFilter: string;
  /**
   * Which project groups are expanded in the sidebar. Lives here (not in the
   * Sidebar component) so it survives the Sidebar unmount/remount that happens
   * on every tab switch — expansion is a property of the workspace, not of the
   * currently visible tab.
   */
  expanded: Record<string, boolean>;
  /** Sidebar layout: project tree vs flat topic list. Persisted. */
  sidebarView: SidebarView;
  refresh: () => Promise<void>;
  setActive: (id: string | null) => void;
  setWorkspaceFilter: (workspace: string) => void;
  /** Flip between the project tree and the flat topic list. */
  toggleSidebarView: () => void;
  /** Flip the expanded state of one project group. */
  toggleExpanded: (id: string) => void;
  /** Force a project group open or closed. */
  setExpanded: (id: string, value: boolean) => void;
  /** Lookup the full row by id; `null` if not found. */
  active: () => ProjectRow | null;
}

export const useProjectStore = create<ProjectStoreState>((set, get) => ({
  projects: [],
  activeId: null,
  loading: false,
  error: null,
  workspaceFilter: loadWorkspaceFilter(),
  expanded: {},
  sidebarView: loadSidebarView(),

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

  setWorkspaceFilter: (workspace) => {
    try {
      window.localStorage.setItem(WORKSPACE_FILTER_KEY, workspace);
    } catch {
      /* localStorage may be disabled in odd contexts */
    }
    set({ workspaceFilter: workspace });
  },

  toggleSidebarView: () =>
    set((s) => {
      const next: SidebarView = s.sidebarView === "tree" ? "topics" : "tree";
      try {
        window.localStorage.setItem(SIDEBAR_VIEW_KEY, next);
      } catch {
        /* localStorage may be disabled in odd contexts */
      }
      return { sidebarView: next };
    }),

  toggleExpanded: (id) =>
    set((s) => ({ expanded: { ...s.expanded, [id]: !s.expanded[id] } })),

  setExpanded: (id, value) =>
    set((s) =>
      s.expanded[id] === value
        ? s
        : { expanded: { ...s.expanded, [id]: value } },
    ),

  active: () => {
    const { projects, activeId } = get();
    if (!activeId) return null;
    return projects.find((p) => p.id === activeId) ?? null;
  },
}));

/**
 * Distinct, sorted workspace labels present across projects. Ungrouped
 * projects (null/empty workspace) do not contribute a label.
 */
export function workspacesOf(projects: ProjectRow[]): string[] {
  const set = new Set<string>();
  for (const p of projects) {
    const ws = (p.workspace ?? "").trim();
    if (ws) set.add(ws);
  }
  return [...set].sort((a, b) => a.localeCompare(b));
}
