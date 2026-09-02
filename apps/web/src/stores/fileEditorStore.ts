import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import {
  fsCopy,
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
 * Project- and worktree-scoped state for the file tree + editor.
 *
 * Every map below is keyed by {@link scopeKey} — `"<projectId>::<worktreeId>"`.
 * The worktree id alone is NOT enough: the primary checkout uses the same
 * nil-UUID sentinel for *every* project, so keying by worktree id alone made
 * two projects' primary trees collide (project A's files showing under project
 * B). The composite key keeps each project's primary checkout isolated while
 * still preserving per-worktree state within a project.
 */

/** Composite store key — see the module doc. */
export function scopeKey(projectId: string, worktreeId: string): string {
  return `${projectId}::${worktreeId}`;
}

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
  /** On-disk content captured by the external-change watcher when it diverged
   *  from {@link baseContent} *and* the buffer has unsaved edits — drives the
   *  reload/keep-mine conflict banner. `null` when there's no pending external
   *  change (a clean buffer is reloaded silently instead). */
  externalContent: string | null;
};

/** A file/folder marked for copy or move via the tree context menu. */
export type FileClipboard = {
  projectId: string;
  worktreeId: string;
  relPath: string;
  op: "copy" | "cut";
} | null;

interface FileEditorState {
  /** scopeKey → relPath → DirNode (expanded folders). */
  trees: Record<string, Record<string, DirNode>>;
  /** scopeKey → set of expanded relPaths (preserves user expand state). */
  expanded: Record<string, Record<string, boolean>>;
  /** scopeKey → ordered list of open tabs (by relPath). */
  openOrder: Record<string, string[]>;
  /** scopeKey → relPath → Tab. */
  tabs: Record<string, Record<string, Tab>>;
  /** scopeKey → currently focused tab relPath (or null). */
  active: Record<string, string | null>;
  /** scopeKey → pending "scroll the editor to this line" request. The `nonce`
   *  lets the same (file,line) be re-requested and re-fire the editor effect.
   *  Never persisted. */
  reveal: Record<
    string,
    { relPath: string; line: number; nonce: number } | null
  >;

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
  /** Open a file (if not already) and request the editor scroll to `line`
   *  (1-based). Used by symbol search and Find-in-Files navigation. */
  openFileAt: (
    projectId: string,
    worktreeId: string,
    relPath: string,
    line: number,
  ) => Promise<void>;
  /** Clear a consumed reveal request so it won't re-fire on remount. */
  consumeReveal: (projectId: string, worktreeId: string) => void;
  closeTab: (projectId: string, worktreeId: string, relPath: string) => void;
  closeOthers: (
    projectId: string,
    worktreeId: string,
    keepRelPath: string,
  ) => void;
  closeAll: (projectId: string, worktreeId: string) => void;
  setActive: (
    projectId: string,
    worktreeId: string,
    relPath: string | null,
  ) => void;
  setBuffer: (
    projectId: string,
    worktreeId: string,
    relPath: string,
    buffer: string,
  ) => void;
  saveTab: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  /** Re-read a tab that came back truncated, this time with a cap high enough
   *  to hold the whole file, so it becomes editable/savable. */
  loadFullFile: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  /** Reconcile an open tab against an external (on-disk) change reported by
   *  the fs watcher. Clean buffers are reloaded silently; dirty buffers raise
   *  a conflict (sets `externalContent` → reload/keep banner). No-op when the
   *  disk still matches `baseContent` (our own save / watcher noise). */
  reconcileExternalChange: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  /** Discard local edits and adopt the on-disk content (resolves a conflict). */
  reloadFromDisk: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => Promise<void>;
  /** Keep local edits, acknowledging the on-disk version as the new base so a
   *  subsequent save overwrites it (resolves a conflict). */
  keepLocalChanges: (
    projectId: string,
    worktreeId: string,
    relPath: string,
  ) => void;
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

  /** File/folder marked for copy or move; null when empty. */
  clipboard: FileClipboard;
  setClipboard: (clip: FileClipboard) => void;
  /** Paste the clipboard entry into `destDir` (worktree-relative, "" = root).
   *  Copy duplicates, cut moves and clears the clipboard. Resolves name
   *  collisions by suffixing " copy". */
  pasteInto: (
    projectId: string,
    worktreeId: string,
    destDir: string,
  ) => Promise<void>;
}

/** Read cap for "load the whole file": high enough for anything worth opening
 *  in the editor, low enough that a stray multi-GB file can't wedge the
 *  WebView. Files above it stay truncated → read-only. */
const FULL_READ_CAP = 16 * 1024 * 1024;

/** Best-effort string from a thrown value — Tauri rejections are often plain
 *  objects/strings, not `Error`s, so `String(e)` would yield "[object Object]". */
function errMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  if (e && typeof e === "object") {
    const m = (e as { message?: unknown }).message;
    if (typeof m === "string") return m;
    try {
      return JSON.stringify(e);
    } catch {
      /* fall through */
    }
  }
  return String(e);
}

export const useFileEditorStore = create<FileEditorState>()(
  persist(
    (set, get) => ({
  trees: {},
  expanded: {},
  openOrder: {},
  tabs: {},
  active: {},
  reveal: {},
  clipboard: null,

  loadDir: async (projectId, worktreeId, relPath) => {
    const key = scopeKey(projectId, worktreeId);
    set((state) => {
      const wtTrees = { ...(state.trees[key] ?? {}) };
      const prev = wtTrees[relPath];
      wtTrees[relPath] = {
        relPath,
        children: prev?.children ?? null,
        loading: true,
        error: null,
      };
      return { trees: { ...state.trees, [key]: wtTrees } };
    });
    try {
      const result = await fsListDir({ projectId, worktreeId, relPath });
      set((state) => {
        const wtTrees = { ...(state.trees[key] ?? {}) };
        wtTrees[relPath] = {
          relPath,
          children: result.entries,
          loading: false,
          error: null,
        };
        return { trees: { ...state.trees, [key]: wtTrees } };
      });
    } catch (e) {
      const message = errMessage(e);
      set((state) => {
        const wtTrees = { ...(state.trees[key] ?? {}) };
        const prev = wtTrees[relPath];
        wtTrees[relPath] = {
          relPath,
          children: prev?.children ?? null,
          loading: false,
          error: message,
        };
        return { trees: { ...state.trees, [key]: wtTrees } };
      });
    }
  },

  toggleExpand: async (projectId, worktreeId, relPath) => {
    const key = scopeKey(projectId, worktreeId);
    const wtExpanded = get().expanded[key] ?? {};
    const next = !wtExpanded[relPath];
    set((state) => ({
      expanded: {
        ...state.expanded,
        [key]: { ...wtExpanded, [relPath]: next },
      },
    }));
    if (next) {
      const cached = get().trees[key]?.[relPath]?.children;
      if (!cached) {
        await get().loadDir(projectId, worktreeId, relPath);
      }
    }
  },

  openFile: async (projectId, worktreeId, relPath) => {
    const key = scopeKey(projectId, worktreeId);
    const existing = get().tabs[key]?.[relPath];
    if (existing) {
      get().setActive(projectId, worktreeId, relPath);
      return;
    }
    const kind = previewKindFor(relPath);
    const isBinary = kind === "image" || kind === "pdf";
    set((state) => {
      const order = state.openOrder[key] ?? [];
      const tabs = state.tabs[key] ?? {};
      return {
        openOrder: {
          ...state.openOrder,
          [key]: order.includes(relPath) ? order : [...order, relPath],
        },
        tabs: {
          ...state.tabs,
          [key]: {
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
              externalContent: null,
            },
          },
        },
        active: { ...state.active, [key]: relPath },
      };
    });
    if (isBinary) return;
    try {
      const result = await fsReadFile({ projectId, worktreeId, relPath });
      set((state) => {
        const tabs = { ...(state.tabs[key] ?? {}) };
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
          externalContent: null,
          ...(prev?.buffer && prev.buffer !== prev.baseContent
            ? { buffer: prev.buffer }
            : {}),
        };
        return { tabs: { ...state.tabs, [key]: tabs } };
      });
    } catch (e) {
      const message = errMessage(e);
      set((state) => {
        const tabs = { ...(state.tabs[key] ?? {}) };
        tabs[relPath] = {
          relPath,
          baseContent: "",
          buffer: "",
          loading: false,
          saving: false,
          error: message,
          truncated: false,
          kind,
          externalContent: null,
        };
        return { tabs: { ...state.tabs, [key]: tabs } };
      });
    }
  },

  openFileAt: async (projectId, worktreeId, relPath, line) => {
    await get().openFile(projectId, worktreeId, relPath);
    const key = scopeKey(projectId, worktreeId);
    set((state) => ({
      reveal: {
        ...state.reveal,
        [key]: {
          relPath,
          line,
          nonce: (state.reveal[key]?.nonce ?? 0) + 1,
        },
      },
    }));
  },

  consumeReveal: (projectId, worktreeId) =>
    set((state) => {
      const key = scopeKey(projectId, worktreeId);
      if (!state.reveal[key]) return state;
      return { reveal: { ...state.reveal, [key]: null } };
    }),

  closeTab: (projectId, worktreeId, relPath) =>
    set((state) => {
      const key = scopeKey(projectId, worktreeId);
      const order = (state.openOrder[key] ?? []).filter((p) => p !== relPath);
      const tabs = { ...(state.tabs[key] ?? {}) };
      delete tabs[relPath];
      const wasActive = state.active[key] === relPath;
      const nextActive = wasActive
        ? (order[order.length - 1] ?? null)
        : state.active[key];
      return {
        openOrder: { ...state.openOrder, [key]: order },
        tabs: { ...state.tabs, [key]: tabs },
        active: { ...state.active, [key]: nextActive ?? null },
      };
    }),

  closeOthers: (projectId, worktreeId, keepRelPath) =>
    set((state) => {
      const key = scopeKey(projectId, worktreeId);
      return {
        openOrder: { ...state.openOrder, [key]: [keepRelPath] },
        tabs: {
          ...state.tabs,
          [key]: state.tabs[key]?.[keepRelPath]
            ? { [keepRelPath]: state.tabs[key]![keepRelPath]! }
            : {},
        },
        active: { ...state.active, [key]: keepRelPath },
      };
    }),

  closeAll: (projectId, worktreeId) =>
    set((state) => {
      const key = scopeKey(projectId, worktreeId);
      return {
        openOrder: { ...state.openOrder, [key]: [] },
        tabs: { ...state.tabs, [key]: {} },
        active: { ...state.active, [key]: null },
      };
    }),

  setActive: (projectId, worktreeId, relPath) =>
    set((state) => ({
      active: { ...state.active, [scopeKey(projectId, worktreeId)]: relPath },
    })),

  setBuffer: (projectId, worktreeId, relPath, buffer) =>
    set((state) => {
      const key = scopeKey(projectId, worktreeId);
      const tabs = { ...(state.tabs[key] ?? {}) };
      const prev = tabs[relPath];
      if (!prev) return state;
      tabs[relPath] = { ...prev, buffer };
      return { tabs: { ...state.tabs, [key]: tabs } };
    }),

  saveTab: async (projectId, worktreeId, relPath) => {
    const key = scopeKey(projectId, worktreeId);
    const tab = get().tabs[key]?.[relPath];
    if (!tab || tab.saving) return;
    // The buffer only holds the first `max_bytes` of the file — writing it
    // back would drop everything past the cut. The editor is read-only in this
    // state; this is the backstop for any other caller.
    if (tab.truncated) return;
    set((state) => {
      const tabs = { ...(state.tabs[key] ?? {}) };
      tabs[relPath] = { ...tab, saving: true, error: null };
      return { tabs: { ...state.tabs, [key]: tabs } };
    });
    try {
      await fsWriteFile({
        projectId,
        worktreeId,
        relPath,
        content: tab.buffer,
      });
      set((state) => {
        const tabs = { ...(state.tabs[key] ?? {}) };
        const cur = tabs[relPath];
        if (!cur) return state;
        tabs[relPath] = {
          ...cur,
          baseContent: cur.buffer,
          saving: false,
          error: null,
        };
        return { tabs: { ...state.tabs, [key]: tabs } };
      });
    } catch (e) {
      const message = errMessage(e);
      set((state) => {
        const tabs = { ...(state.tabs[key] ?? {}) };
        const cur = tabs[relPath];
        if (!cur) return state;
        tabs[relPath] = { ...cur, saving: false, error: message };
        return { tabs: { ...state.tabs, [key]: tabs } };
      });
    }
  },

  loadFullFile: async (projectId, worktreeId, relPath) => {
    const key = scopeKey(projectId, worktreeId);
    const tab = get().tabs[key]?.[relPath];
    if (!tab || tab.loading || tab.saving) return;
    // Re-reading replaces the buffer. A truncated tab is read-only so it
    // normally can't be dirty, but the rehydrate path carries a buffer over —
    // never drop edits to widen the read.
    if (tab.buffer !== tab.baseContent) return;
    set((state) => {
      const tabs = { ...(state.tabs[key] ?? {}) };
      const cur = tabs[relPath];
      if (!cur) return state;
      tabs[relPath] = { ...cur, loading: true, error: null };
      return { tabs: { ...state.tabs, [key]: tabs } };
    });
    try {
      const result = await fsReadFile({
        projectId,
        worktreeId,
        relPath,
        maxBytes: FULL_READ_CAP,
      });
      set((state) => {
        const tabs = { ...(state.tabs[key] ?? {}) };
        const cur = tabs[relPath];
        if (!cur) return state;
        tabs[relPath] = {
          ...cur,
          baseContent: result.content,
          buffer: result.content,
          truncated: result.truncated,
          loading: false,
          // Still truncated (a genuinely huge file): the tab keeps its
          // read-only banner. No error string here — the store holds no
          // user-facing copy, the banner is rendered from `truncated`.
          error: null,
          externalContent: null,
        };
        return { tabs: { ...state.tabs, [key]: tabs } };
      });
    } catch (e) {
      const message = errMessage(e);
      set((state) => {
        const tabs = { ...(state.tabs[key] ?? {}) };
        const cur = tabs[relPath];
        if (!cur) return state;
        tabs[relPath] = { ...cur, loading: false, error: message };
        return { tabs: { ...state.tabs, [key]: tabs } };
      });
    }
  },

  reconcileExternalChange: async (projectId, worktreeId, relPath) => {
    const key = scopeKey(projectId, worktreeId);
    const tab = get().tabs[key]?.[relPath];
    if (!tab) return;
    // Binary previews load their own bytes via fs_read_file_bytes; nothing to
    // reconcile here. Skip while a read/write is already in flight.
    if (tab.kind === "image" || tab.kind === "pdf") return;
    if (tab.loading || tab.saving) return;

    let disk: { content: string; truncated: boolean };
    try {
      const r = await fsReadFile({ projectId, worktreeId, relPath });
      disk = { content: r.content, truncated: r.truncated };
    } catch {
      // File vanished or became unreadable — leave the tab untouched. A delete
      // is handled separately by the tree refresh / closeTab paths.
      return;
    }
    set((state) => {
      const tabs = { ...(state.tabs[key] ?? {}) };
      const cur = tabs[relPath];
      if (!cur) return state;
      // Disk matches what we last synced → no real external change (our own
      // save echoing back through the watcher, or unrelated noise). Drop any
      // stale conflict flag.
      if (disk.content === cur.baseContent) {
        if (cur.externalContent === null) return state;
        tabs[relPath] = { ...cur, externalContent: null };
        return { tabs: { ...state.tabs, [key]: tabs } };
      }
      const dirty = cur.buffer !== cur.baseContent;
      if (!dirty) {
        // Clean buffer → silently adopt the new on-disk content (the user
        // asked for unmodified files to just refresh in place).
        tabs[relPath] = {
          ...cur,
          baseContent: disk.content,
          buffer: disk.content,
          truncated: disk.truncated,
          externalContent: null,
        };
      } else {
        // Unsaved edits AND disk diverged → surface a reload/keep conflict.
        tabs[relPath] = { ...cur, externalContent: disk.content };
      }
      return { tabs: { ...state.tabs, [key]: tabs } };
    });
  },

  reloadFromDisk: async (projectId, worktreeId, relPath) => {
    const key = scopeKey(projectId, worktreeId);
    const tab = get().tabs[key]?.[relPath];
    if (!tab) return;
    // Prefer the content the watcher already captured; otherwise read fresh.
    let content = tab.externalContent;
    let truncated = tab.truncated;
    if (content === null) {
      try {
        const r = await fsReadFile({ projectId, worktreeId, relPath });
        content = r.content;
        truncated = r.truncated;
      } catch (e) {
        const message = errMessage(e);
        set((state) => {
          const tabs = { ...(state.tabs[key] ?? {}) };
          const cur = tabs[relPath];
          if (!cur) return state;
          tabs[relPath] = { ...cur, error: message };
          return { tabs: { ...state.tabs, [key]: tabs } };
        });
        return;
      }
    }
    const next = content;
    set((state) => {
      const tabs = { ...(state.tabs[key] ?? {}) };
      const cur = tabs[relPath];
      if (!cur) return state;
      tabs[relPath] = {
        ...cur,
        baseContent: next,
        buffer: next,
        truncated,
        externalContent: null,
        error: null,
      };
      return { tabs: { ...state.tabs, [key]: tabs } };
    });
  },

  keepLocalChanges: (projectId, worktreeId, relPath) =>
    set((state) => {
      const key = scopeKey(projectId, worktreeId);
      const tabs = { ...(state.tabs[key] ?? {}) };
      const cur = tabs[relPath];
      if (!cur || cur.externalContent === null) return state;
      // Adopt the on-disk version as the new base so the editor still shows
      // the tab as dirty and the next save overwrites disk with the buffer.
      tabs[relPath] = {
        ...cur,
        baseContent: cur.externalContent,
        externalContent: null,
      };
      return { tabs: { ...state.tabs, [key]: tabs } };
    }),

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
      const key = scopeKey(projectId, worktreeId);
      const order = state.openOrder[key] ?? [];
      if (!order.includes(fromRel)) return state;
      const newOrder = order.map((p) => (p === fromRel ? toRel : p));
      const tabs = { ...(state.tabs[key] ?? {}) };
      const oldTab = tabs[fromRel];
      if (oldTab) {
        tabs[toRel] = { ...oldTab, relPath: toRel };
        delete tabs[fromRel];
      }
      const active = state.active[key] === fromRel ? toRel : state.active[key];
      return {
        openOrder: { ...state.openOrder, [key]: newOrder },
        tabs: { ...state.tabs, [key]: tabs },
        active: { ...state.active, [key]: active ?? null },
      };
    });
  },

  deleteEntry: async (projectId, worktreeId, relPath, recursive) => {
    await fsDelete({ projectId, worktreeId, relPath, recursive });
    await get().loadDir(projectId, worktreeId, parentDir(relPath));
    // If the deleted file was open, drop its tab.
    const key = scopeKey(projectId, worktreeId);
    const order = get().openOrder[key] ?? [];
    if (order.includes(relPath)) {
      get().closeTab(projectId, worktreeId, relPath);
    }
  },

  setClipboard: (clip) => set({ clipboard: clip }),

  pasteInto: async (projectId, worktreeId, destDir) => {
    const clip = get().clipboard;
    if (!clip) return;
    const key = scopeKey(projectId, worktreeId);
    // Block pasting a folder into itself or one of its descendants (would
    // recurse / corrupt). Compare against the normalized dest path.
    if (
      clip.op === "cut" &&
      (destDir === parentDir(clip.relPath) ||
        destDir === clip.relPath ||
        destDir.startsWith(`${clip.relPath}/`))
    ) {
      return;
    }
    // Make sure we know the destination's contents so we can dodge name
    // collisions before writing.
    if (!get().trees[key]?.[destDir]?.children) {
      await get().loadDir(projectId, worktreeId, destDir);
    }
    const siblings = get().trees[key]?.[destDir]?.children ?? [];
    const taken = new Set(siblings.map((s) => s.name));
    const targetName = uniqueName(basename(clip.relPath), taken);
    const target = destDir ? `${destDir}/${targetName}` : targetName;

    if (clip.op === "cut") {
      await fsRename({ projectId, worktreeId, fromRel: clip.relPath, toRel: target });
      await get().loadDir(projectId, worktreeId, parentDir(clip.relPath));
      // Keep an open tab pointing at the moved file.
      set((state) => {
        const order = state.openOrder[key] ?? [];
        if (!order.includes(clip.relPath)) return { clipboard: null };
        const newOrder = order.map((p) => (p === clip.relPath ? target : p));
        const tabs = { ...(state.tabs[key] ?? {}) };
        const oldTab = tabs[clip.relPath];
        if (oldTab) {
          tabs[target] = { ...oldTab, relPath: target };
          delete tabs[clip.relPath];
        }
        const active =
          state.active[key] === clip.relPath ? target : state.active[key];
        return {
          openOrder: { ...state.openOrder, [key]: newOrder },
          tabs: { ...state.tabs, [key]: tabs },
          active: { ...state.active, [key]: active ?? null },
          clipboard: null,
        };
      });
    } else {
      await fsCopy({ projectId, worktreeId, fromRel: clip.relPath, toRel: target });
    }
    await get().loadDir(projectId, worktreeId, destDir);
  },

  subscribeFsChanged: async (projectIdResolver) => {
    return await listen<{ worktree_id: string; paths: string[] }>(
      "fs:changed",
      (e) => {
        const { worktree_id, paths } = e.payload;
        const projectId = projectIdResolver();
        if (!projectId) return;
        const key = scopeKey(projectId, worktree_id);
        // Reconcile any open editor tab whose file changed on disk — reload
        // clean buffers silently, raise a conflict on dirty ones. Useful when
        // an LLM (or another tool) rewrites a file while it's open.
        const openTabs = get().tabs[key];
        if (openTabs) {
          for (const p of paths) {
            if (openTabs[p]) {
              void get().reconcileExternalChange(projectId, worktree_id, p);
            }
          }
        }
        // Refresh each unique parent dir that we already have loaded —
        // skip dirs the user never expanded so we don't fetch the whole
        // tree on every save.
        const trees = get().trees[key];
        if (!trees) return;
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

function basename(relPath: string): string {
  const idx = relPath.lastIndexOf("/");
  return idx >= 0 ? relPath.slice(idx + 1) : relPath;
}

/** Return `name` if free, else suffix " copy" (then " copy 2", …) before the
 *  extension until it doesn't collide with `taken`. */
function uniqueName(name: string, taken: Set<string>): string {
  if (!taken.has(name)) return name;
  const dot = name.lastIndexOf(".");
  const hasExt = dot > 0;
  const stem = hasExt ? name.slice(0, dot) : name;
  const ext = hasExt ? name.slice(dot) : "";
  let candidate = `${stem} copy${ext}`;
  let n = 2;
  while (taken.has(candidate)) {
    candidate = `${stem} copy ${n}${ext}`;
    n += 1;
  }
  return candidate;
}
