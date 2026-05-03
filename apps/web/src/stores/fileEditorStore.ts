import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import {
  fsCreateDir,
  fsCreateFile,
  fsDelete,
  fsListDir,
  fsReadFile,
  fsRename,
  fsWriteFile,
  previewKindFor,
  type FsEntry,
  type PreviewKind,
} from "~/ipc/fs.ts";

/**
 * Worktree-scoped state for the file tree + editor.
 *
 * Every map below is keyed by `worktreeId` so switching between worktrees
 * (or between sessions on different worktrees) preserves expanded folders,
 * open tabs, and dirty buffers per scope. The active worktree is tracked
 * outside this store (`sessionStore`) — readers select state via
 * `useFileEditorStore((s) => s.<map>[worktreeId])`.
 */

export type DirNode = {
  /** Path relative to worktree root, "" for root itself. */
  relPath: string;
  /** Cached children when this directory has been expanded. */
  children: FsEntry[] | null;
  loading: boolean;
  error: string | null;
};

export type Tab = {
  /** Path relative to worktree root. */
  relPath: string;
  /** Last on-disk content the editor synced with. */
  baseContent: string;
  /** Live editor buffer. */
  buffer: string;
  loading: boolean;
  saving: boolean;
  error: string | null;
  truncated: boolean;
  /** What kind of preview/editor to use. Set on open from `previewKindFor`. */
  kind: PreviewKind;
};

interface FileEditorState {
  /** worktreeId → relPath → DirNode (expanded folders). */
  trees: Record<string, Record<string, DirNode>>;
  /** worktreeId → set of expanded relPaths (preserves user expand state). */
  expanded: Record<string, Record<string, boolean>>;
  /** worktreeId → ordered list of open tabs (by relPath). */
  openOrder: Record<string, string[]>;
  /** worktreeId → relPath → Tab. */
  tabs: Record<string, Record<string, Tab>>;
  /** worktreeId → currently focused tab relPath (or null). */
  active: Record<string, string | null>;

  loadDir: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  toggleExpand: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  openFile: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  closeTab: (worktreeId: string, relPath: string) => void;
  closeOthers: (worktreeId: string, keepRelPath: string) => void;
  closeAll: (worktreeId: string) => void;
  setActive: (worktreeId: string, relPath: string | null) => void;
  setBuffer: (worktreeId: string, relPath: string, buffer: string) => void;
  saveTab: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  refreshDir: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  createFile: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  createDir: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  renameEntry: (
    projectId: string,
    worktreeId: string,
    fromRel: string,
    toRel: string,
  ) => Promise<void>;
  deleteEntry: (
    projectId: string,
    worktreeId: string,
    relPath: string,
    recursive: boolean,
  ) => Promise<void>;
  /** Subscribe to backend `fs:changed` events; returns the Tauri unlisten. */
  subscribeFsChanged: (projectIdResolver: () => string | null) => Promise<UnlistenFn>;
}

export const useFileEditorStore = create<FileEditorState>()(
  persist(
    (set, get) => ({
  trees: {},
  expanded: {},
  openOrder: {},
  tabs: {},
  active: {},

  loadDir: async (projectId, worktreeId, relPath) => {
    set((state) => {
      const wtTrees = { ...(state.trees[worktreeId] ?? {}) };
      const prev = wtTrees[relPath];
      wtTrees[relPath] = {
        relPath,
        children: prev?.children ?? null,
        loading: true,
        error: null,
      };
      return { trees: { ...state.trees, [worktreeId]: wtTrees } };
    });
    try {
      const result = await fsListDir({ projectId, worktreeId, relPath });
      set((state) => {
        const wtTrees = { ...(state.trees[worktreeId] ?? {}) };
        wtTrees[relPath] = {
          relPath,
          children: result.entries,
          loading: false,
          error: null,
        };
        return { trees: { ...state.trees, [worktreeId]: wtTrees } };
      });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      set((state) => {
        const wtTrees = { ...(state.trees[worktreeId] ?? {}) };
        const prev = wtTrees[relPath];
        wtTrees[relPath] = {
          relPath,
          children: prev?.children ?? null,
          loading: false,
          error: message,
        };
        return { trees: { ...state.trees, [worktreeId]: wtTrees } };
      });
    }
  },

  toggleExpand: async (projectId, worktreeId, relPath) => {
    const wtExpanded = get().expanded[worktreeId] ?? {};
    const next = !wtExpanded[relPath];
    set((state) => ({
      expanded: {
        ...state.expanded,
        [worktreeId]: { ...wtExpanded, [relPath]: next },
      },
    }));
    if (next) {
      const cached = get().trees[worktreeId]?.[relPath]?.children;
      if (!cached) {
        await get().loadDir(projectId, worktreeId, relPath);
      }
    }
  },

  openFile: async (projectId, worktreeId, relPath) => {
    const existing = get().tabs[worktreeId]?.[relPath];
    if (existing) {
      get().setActive(worktreeId, relPath);
      return;
    }
    const kind = previewKindFor(relPath);
    const isBinary = kind === "image" || kind === "pdf";
    set((state) => {
      const order = state.openOrder[worktreeId] ?? [];
      const tabs = state.tabs[worktreeId] ?? {};
      return {
        openOrder: {
          ...state.openOrder,
          [worktreeId]: order.includes(relPath) ? order : [...order, relPath],
        },
        tabs: {
          ...state.tabs,
          [worktreeId]: {
            ...tabs,
            [relPath]: {
              relPath,
              baseContent: "",
              buffer: "",
              // Binary previews load themselves via fs_read_file_bytes —
              // skip the text read to avoid corrupting bytes through the
              // utf-8 lossy round-trip.
              loading: !isBinary,
              saving: false,
              error: null,
              truncated: false,
              kind,
            },
          },
        },
        active: { ...state.active, [worktreeId]: relPath },
      };
    });
    if (isBinary) return;
    try {
      const result = await fsReadFile({ projectId, worktreeId, relPath });
      set((state) => {
        const tabs = { ...(state.tabs[worktreeId] ?? {}) };
        const prev = tabs[relPath];
        tabs[relPath] = {
          relPath,
          baseContent: result.content,
          buffer: result.content,
          loading: false,
          saving: false,
          error: null,
          truncated: result.truncated,
          kind,
          ...(prev?.buffer && prev.buffer !== prev.baseContent
            ? { buffer: prev.buffer }
            : {}),
        };
        return { tabs: { ...state.tabs, [worktreeId]: tabs } };
      });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      set((state) => {
        const tabs = { ...(state.tabs[worktreeId] ?? {}) };
        tabs[relPath] = {
          relPath,
          baseContent: "",
          buffer: "",
          loading: false,
          saving: false,
          error: message,
          truncated: false,
          kind,
        };
        return { tabs: { ...state.tabs, [worktreeId]: tabs } };
      });
    }
  },

  closeTab: (worktreeId, relPath) =>
    set((state) => {
      const order = (state.openOrder[worktreeId] ?? []).filter(
        (p) => p !== relPath,
      );
      const tabs = { ...(state.tabs[worktreeId] ?? {}) };
      delete tabs[relPath];
      const wasActive = state.active[worktreeId] === relPath;
      const nextActive = wasActive
        ? (order[order.length - 1] ?? null)
        : state.active[worktreeId];
      return {
        openOrder: { ...state.openOrder, [worktreeId]: order },
        tabs: { ...state.tabs, [worktreeId]: tabs },
        active: { ...state.active, [worktreeId]: nextActive ?? null },
      };
    }),

  closeOthers: (worktreeId, keepRelPath) =>
    set((state) => ({
      openOrder: { ...state.openOrder, [worktreeId]: [keepRelPath] },
      tabs: {
        ...state.tabs,
        [worktreeId]: state.tabs[worktreeId]?.[keepRelPath]
          ? { [keepRelPath]: state.tabs[worktreeId]![keepRelPath]! }
          : {},
      },
      active: { ...state.active, [worktreeId]: keepRelPath },
    })),

  closeAll: (worktreeId) =>
    set((state) => ({
      openOrder: { ...state.openOrder, [worktreeId]: [] },
      tabs: { ...state.tabs, [worktreeId]: {} },
      active: { ...state.active, [worktreeId]: null },
    })),

  setActive: (worktreeId, relPath) =>
    set((state) => ({
      active: { ...state.active, [worktreeId]: relPath },
    })),

  setBuffer: (worktreeId, relPath, buffer) =>
    set((state) => {
      const tabs = { ...(state.tabs[worktreeId] ?? {}) };
      const prev = tabs[relPath];
      if (!prev) return state;
      tabs[relPath] = { ...prev, buffer };
      return { tabs: { ...state.tabs, [worktreeId]: tabs } };
    }),

  saveTab: async (projectId, worktreeId, relPath) => {
    const tab = get().tabs[worktreeId]?.[relPath];
    if (!tab || tab.saving) return;
    set((state) => {
      const tabs = { ...(state.tabs[worktreeId] ?? {}) };
      tabs[relPath] = { ...tab, saving: true, error: null };
      return { tabs: { ...state.tabs, [worktreeId]: tabs } };
    });
    try {
      await fsWriteFile({
        projectId,
        worktreeId,
        relPath,
        content: tab.buffer,
      });
      set((state) => {
        const tabs = { ...(state.tabs[worktreeId] ?? {}) };
        const cur = tabs[relPath];
        if (!cur) return state;
        tabs[relPath] = {
          ...cur,
          baseContent: cur.buffer,
          saving: false,
          error: null,
        };
        return { tabs: { ...state.tabs, [worktreeId]: tabs } };
      });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      set((state) => {
        const tabs = { ...(state.tabs[worktreeId] ?? {}) };
        const cur = tabs[relPath];
        if (!cur) return state;
        tabs[relPath] = { ...cur, saving: false, error: message };
        return { tabs: { ...state.tabs, [worktreeId]: tabs } };
      });
    }
  },

  refreshDir: async (projectId, worktreeId, relPath) => {
    await get().loadDir(projectId, worktreeId, relPath);
  },

  createFile: async (projectId, worktreeId, relPath) => {
    await fsCreateFile({ projectId, worktreeId, relPath });
    await get().loadDir(projectId, worktreeId, parentDir(relPath));
  },

  createDir: async (projectId, worktreeId, relPath) => {
    await fsCreateDir({ projectId, worktreeId, relPath });
    await get().loadDir(projectId, worktreeId, parentDir(relPath));
  },

  renameEntry: async (projectId, worktreeId, fromRel, toRel) => {
    await fsRename({ projectId, worktreeId, fromRel, toRel });
    // Refresh both parent dirs (they may differ when moving across folders).
    const fromParent = parentDir(fromRel);
    const toParent = parentDir(toRel);
    await get().loadDir(projectId, worktreeId, fromParent);
    if (toParent !== fromParent) {
      await get().loadDir(projectId, worktreeId, toParent);
    }
    // If the renamed file was open, swap its tab key.
    set((state) => {
      const order = state.openOrder[worktreeId] ?? [];
      if (!order.includes(fromRel)) return state;
      const newOrder = order.map((p) => (p === fromRel ? toRel : p));
      const tabs = { ...(state.tabs[worktreeId] ?? {}) };
      const oldTab = tabs[fromRel];
      if (oldTab) {
        tabs[toRel] = { ...oldTab, relPath: toRel };
        delete tabs[fromRel];
      }
      const active = state.active[worktreeId] === fromRel ? toRel : state.active[worktreeId];
      return {
        openOrder: { ...state.openOrder, [worktreeId]: newOrder },
        tabs: { ...state.tabs, [worktreeId]: tabs },
        active: { ...state.active, [worktreeId]: active ?? null },
      };
    });
  },

  deleteEntry: async (projectId, worktreeId, relPath, recursive) => {
    await fsDelete({ projectId, worktreeId, relPath, recursive });
    await get().loadDir(projectId, worktreeId, parentDir(relPath));
    // If the deleted file was open, drop its tab.
    const order = get().openOrder[worktreeId] ?? [];
    if (order.includes(relPath)) {
      get().closeTab(worktreeId, relPath);
    }
  },

  subscribeFsChanged: async (projectIdResolver) => {
    return await listen<{ worktree_id: string; paths: string[] }>(
      "fs:changed",
      (e) => {
        const { worktree_id, paths } = e.payload;
        // Refresh each unique parent dir that we already have loaded —
        // skip dirs the user never expanded so we don't fetch the whole
        // tree on every save.
        const trees = get().trees[worktree_id];
        if (!trees) return;
        const projectId = projectIdResolver();
        if (!projectId) return;
        const seen = new Set<string>();
        for (const p of paths) {
          const parent = parentDir(p);
          if (seen.has(parent)) continue;
          seen.add(parent);
          if (trees[parent]) {
            void get().loadDir(projectId, worktree_id, parent);
          }
        }
      },
    );
  },
    }),
    {
      name: "oxyris-file-editor",
      storage: createJSONStorage(() => localStorage),
      // Only persist the lightweight, non-stale-prone slices: which tabs were
      // open and which folders were expanded. Buffers/contents/loading flags
      // are re-fetched on demand so we never resurrect a stale or in-flight
      // file from disk.
      partialize: (state) => ({
        openOrder: state.openOrder,
        active: state.active,
        expanded: state.expanded,
      }),
    },
  ),
);

export function joinPath(parent: string, name: string): string {
  if (!parent) return name;
  return `${parent}/${name}`;
}

function parentDir(relPath: string): string {
  const idx = relPath.lastIndexOf("/");
  return idx >= 0 ? relPath.slice(0, idx) : "";
}
