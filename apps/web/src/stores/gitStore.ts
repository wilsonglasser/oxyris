import { create } from "zustand";
import {
  gitApplyPatch,
  gitBranchCreate,
  gitBranchDelete,
  gitCheckout,
  gitCherryPick,
  gitCommit,
  gitDiffFile,
  gitFetch,
  gitGenerateCommitMessage,
  gitLog,
  gitPull,
  gitPush,
  gitRevert,
  gitStage,
  gitStashApply,
  gitStashDrop,
  gitStashList,
  gitStashSave,
  gitStatus,
  gitTagCreate,
  gitTagDelete,
  gitTagList,
  gitUnstage,
  type CommitInfo,
  type DiffMode,
  type FileDiff,
  type StashEntry,
  type StatusEntry,
  type StatusReport,
  type TagInfo,
} from "~/ipc/git.ts";
import { gitListBranches, type BranchInfo } from "~/ipc/worktree.ts";

/**
 * Worktree-scoped git state.
 *
 * Maps keyed by `worktreeId` so swapping worktrees keeps the panel's view
 * (current diff selection, last status snapshot) per scope. Mutations
 * (stage/unstage/commit) trigger a status refresh for the affected
 * worktree only.
 */

type DiffKey = string; // `${path}::${mode}`

export type SelectedDiff = {
  path: string;
  mode: DiffMode;
};

export type RemoteState = {
  running: boolean;
  lastOutput: string | null;
  error: string | null;
};

interface GitState {
  status: Record<string, StatusReport | null>;
  loading: Record<string, boolean>;
  error: Record<string, string | null>;
  diffs: Record<string, Record<DiffKey, FileDiff>>;
  diffLoading: Record<string, Record<DiffKey, boolean>>;
  selected: Record<string, SelectedDiff | null>;
  commitMessage: Record<string, string>;
  committing: Record<string, boolean>;
  commitError: Record<string, string | null>;
  branches: Record<string, BranchInfo[]>;
  log: Record<string, CommitInfo[]>;
  remote: Record<string, RemoteState>;

  refreshStatus: (projectId: string, worktreeId: string) => Promise<void>;
  selectDiff: (
    projectId: string,
    worktreeId: string,
    path: string,
    mode: DiffMode,
  ) => Promise<void>;
  stagePaths: (
    projectId: string,
    worktreeId: string,
    paths: string[],
  ) => Promise<void>;
  unstagePaths: (
    projectId: string,
    worktreeId: string,
    paths: string[],
  ) => Promise<void>;
  setCommitMessage: (worktreeId: string, msg: string) => void;
  commit: (
    projectId: string,
    worktreeId: string,
    amend?: boolean,
  ) => Promise<void>;
  commitAndPush: (
    projectId: string,
    worktreeId: string,
    amend?: boolean,
  ) => Promise<void>;

  refreshBranches: (projectId: string, worktreeId: string) => Promise<void>;
  refreshLog: (
    projectId: string,
    worktreeId: string,
    limit?: number,
  ) => Promise<void>;
  fetch: (projectId: string, worktreeId: string) => Promise<void>;
  pull: (
    projectId: string,
    worktreeId: string,
    rebase?: boolean,
  ) => Promise<void>;
  push: (
    projectId: string,
    worktreeId: string,
    setUpstream?: boolean,
  ) => Promise<void>;
  checkout: (
    projectId: string,
    worktreeId: string,
    name: string,
  ) => Promise<void>;
  createBranch: (
    projectId: string,
    worktreeId: string,
    name: string,
    checkout?: boolean,
  ) => Promise<void>;
  deleteBranch: (
    projectId: string,
    worktreeId: string,
    name: string,
  ) => Promise<void>;
  generatingCommitMsg: Record<string, boolean>;
  generateCommitMessage: (
    projectId: string,
    worktreeId: string,
  ) => Promise<void>;
  applyHunk: (
    projectId: string,
    worktreeId: string,
    patch: string,
    reverse: boolean,
  ) => Promise<void>;

  stashes: Record<string, StashEntry[]>;
  refreshStashes: (projectId: string, worktreeId: string) => Promise<void>;
  saveStash: (
    projectId: string,
    worktreeId: string,
    message: string,
    includeUntracked: boolean,
  ) => Promise<void>;
  applyStash: (
    projectId: string,
    worktreeId: string,
    index: number,
    dropAfter: boolean,
  ) => Promise<void>;
  dropStash: (projectId: string, worktreeId: string, index: number) => Promise<void>;

  tags: Record<string, TagInfo[]>;
  refreshTags: (projectId: string, worktreeId: string) => Promise<void>;
  createTag: (
    projectId: string,
    worktreeId: string,
    name: string,
    target?: string,
    message?: string,
  ) => Promise<void>;
  deleteTag: (projectId: string, worktreeId: string, name: string) => Promise<void>;

  cherryPick: (
    projectId: string,
    worktreeId: string,
    oid: string,
  ) => Promise<string | null>;
  revertCommit: (
    projectId: string,
    worktreeId: string,
    oid: string,
  ) => Promise<string | null>;
}

export const useGitStore = create<GitState>((set, get) => ({
  status: {},
  loading: {},
  error: {},
  diffs: {},
  diffLoading: {},
  selected: {},
  commitMessage: {},
  committing: {},
  commitError: {},
  branches: {},
  log: {},
  remote: {},
  generatingCommitMsg: {},
  stashes: {},
  tags: {},

  refreshStatus: async (projectId, worktreeId) => {
    set((state) => ({
      loading: { ...state.loading, [worktreeId]: true },
      error: { ...state.error, [worktreeId]: null },
    }));
    try {
      const report = await gitStatus({ projectId, worktreeId });
      set((state) => ({
        status: { ...state.status, [worktreeId]: report },
        loading: { ...state.loading, [worktreeId]: false },
      }));
      // Refresh the currently selected diff if its file is still in the
      // status report, otherwise drop the selection.
      const sel = get().selected[worktreeId];
      if (sel) {
        const stillThere = report.entries.some((e) => e.path === sel.path);
        if (!stillThere) {
          set((state) => ({
            selected: { ...state.selected, [worktreeId]: null },
          }));
        } else {
          await get().selectDiff(projectId, worktreeId, sel.path, sel.mode);
        }
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      set((state) => ({
        loading: { ...state.loading, [worktreeId]: false },
        error: { ...state.error, [worktreeId]: msg },
      }));
    }
  },

  selectDiff: async (projectId, worktreeId, path, mode) => {
    const key = `${path}::${mode}`;
    set((state) => {
      const wtLoading = { ...(state.diffLoading[worktreeId] ?? {}) };
      wtLoading[key] = true;
      return {
        selected: { ...state.selected, [worktreeId]: { path, mode } },
        diffLoading: { ...state.diffLoading, [worktreeId]: wtLoading },
      };
    });
    try {
      const diff = await gitDiffFile({ projectId, worktreeId, path, mode });
      set((state) => {
        const wtDiffs = { ...(state.diffs[worktreeId] ?? {}) };
        wtDiffs[key] = diff;
        const wtLoading = { ...(state.diffLoading[worktreeId] ?? {}) };
        wtLoading[key] = false;
        return {
          diffs: { ...state.diffs, [worktreeId]: wtDiffs },
          diffLoading: { ...state.diffLoading, [worktreeId]: wtLoading },
        };
      });
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      set((state) => {
        const wtLoading = { ...(state.diffLoading[worktreeId] ?? {}) };
        wtLoading[key] = false;
        return {
          diffLoading: { ...state.diffLoading, [worktreeId]: wtLoading },
          error: { ...state.error, [worktreeId]: msg },
        };
      });
    }
  },

  stagePaths: async (projectId, worktreeId, paths) => {
    if (paths.length === 0) return;
    await gitStage({ projectId, worktreeId, paths });
    await get().refreshStatus(projectId, worktreeId);
  },

  unstagePaths: async (projectId, worktreeId, paths) => {
    if (paths.length === 0) return;
    await gitUnstage({ projectId, worktreeId, paths });
    await get().refreshStatus(projectId, worktreeId);
  },

  setCommitMessage: (worktreeId, msg) =>
    set((state) => ({
      commitMessage: { ...state.commitMessage, [worktreeId]: msg },
    })),

  commit: async (projectId, worktreeId, amend = false) => {
    const message = get().commitMessage[worktreeId] ?? "";
    if (!message.trim()) return;
    set((state) => ({
      committing: { ...state.committing, [worktreeId]: true },
      commitError: { ...state.commitError, [worktreeId]: null },
    }));
    try {
      // `git commit -a` style: when nothing is staged, stage every tracked
      // change + untracked file first so a plain Commit "just works".
      // Amend is left alone — it edits the last commit with whatever is
      // already staged and shouldn't sweep in unrelated changes.
      if (!amend) {
        const status = get().status[worktreeId] ?? null;
        const hasStaged =
          status?.entries.some((e) => e.bucket === "staged") ?? false;
        if (!hasStaged) {
          const paths = stageablePaths(status);
          if (paths.length > 0) {
            await gitStage({ projectId, worktreeId, paths });
          }
        }
      }
      await gitCommit({ projectId, worktreeId, message, amend });
      set((state) => ({
        committing: { ...state.committing, [worktreeId]: false },
        commitMessage: { ...state.commitMessage, [worktreeId]: "" },
      }));
      await get().refreshStatus(projectId, worktreeId);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      set((state) => ({
        committing: { ...state.committing, [worktreeId]: false },
        commitError: { ...state.commitError, [worktreeId]: msg },
      }));
    }
  },

  commitAndPush: async (projectId, worktreeId, amend = false) => {
    await get().commit(projectId, worktreeId, amend);
    // commit() swallows its own failure into commitError — bail before push.
    if (get().commitError[worktreeId]) return;
    await get().push(projectId, worktreeId);
    // push() reports into remote state; mirror any failure under the commit
    // box so it's visible next to the button that triggered it.
    const remoteErr = get().remote[worktreeId]?.error ?? null;
    if (remoteErr) {
      set((state) => ({
        commitError: { ...state.commitError, [worktreeId]: remoteErr },
      }));
    }
  },

  refreshBranches: async (projectId, _worktreeId) => {
    const list = await gitListBranches(projectId);
    set((state) => ({
      branches: { ...state.branches, [_worktreeId]: list },
    }));
  },

  refreshLog: async (projectId, worktreeId, limit = 50) => {
    const entries = await gitLog({ projectId, worktreeId, limit });
    set((state) => ({ log: { ...state.log, [worktreeId]: entries } }));
  },

  fetch: async (projectId, worktreeId) => {
    setRemote(set, worktreeId, { running: true, lastOutput: null, error: null });
    try {
      const r = await gitFetch({ projectId, worktreeId });
      setRemote(set, worktreeId, {
        running: false,
        lastOutput: r.stderr || r.stdout,
        error: null,
      });
      await get().refreshStatus(projectId, worktreeId);
      await get().refreshBranches(projectId, worktreeId);
    } catch (e) {
      setRemote(set, worktreeId, {
        running: false,
        lastOutput: null,
        error: errMsg(e),
      });
    }
  },

  pull: async (projectId, worktreeId, rebase = false) => {
    setRemote(set, worktreeId, { running: true, lastOutput: null, error: null });
    try {
      const r = await gitPull({ projectId, worktreeId, rebase });
      setRemote(set, worktreeId, {
        running: false,
        lastOutput: r.stderr || r.stdout,
        error: null,
      });
      await get().refreshStatus(projectId, worktreeId);
    } catch (e) {
      setRemote(set, worktreeId, {
        running: false,
        lastOutput: null,
        error: errMsg(e),
      });
    }
  },

  push: async (projectId, worktreeId, setUpstream = false) => {
    setRemote(set, worktreeId, { running: true, lastOutput: null, error: null });
    try {
      const r = await gitPush({ projectId, worktreeId, setUpstream });
      setRemote(set, worktreeId, {
        running: false,
        lastOutput: r.stderr || r.stdout,
        error: null,
      });
      await get().refreshStatus(projectId, worktreeId);
    } catch (e) {
      setRemote(set, worktreeId, {
        running: false,
        lastOutput: null,
        error: errMsg(e),
      });
    }
  },

  checkout: async (projectId, worktreeId, name) => {
    await gitCheckout({ projectId, worktreeId, name });
    await get().refreshStatus(projectId, worktreeId);
    await get().refreshBranches(projectId, worktreeId);
  },

  createBranch: async (projectId, worktreeId, name, checkout = true) => {
    await gitBranchCreate({ projectId, worktreeId, name, checkout });
    await get().refreshStatus(projectId, worktreeId);
    await get().refreshBranches(projectId, worktreeId);
  },

  deleteBranch: async (projectId, worktreeId, name) => {
    await gitBranchDelete({ projectId, worktreeId, name });
    await get().refreshBranches(projectId, worktreeId);
  },

  refreshStashes: async (projectId, worktreeId) => {
    try {
      const list = await gitStashList({ projectId, worktreeId });
      set((state) => ({ stashes: { ...state.stashes, [worktreeId]: list } }));
    } catch (e) {
      console.warn(e);
    }
  },

  saveStash: async (projectId, worktreeId, message, includeUntracked) => {
    await gitStashSave({ projectId, worktreeId, message, includeUntracked });
    await get().refreshStatus(projectId, worktreeId);
    await get().refreshStashes(projectId, worktreeId);
  },

  applyStash: async (projectId, worktreeId, index, dropAfter) => {
    await gitStashApply({ projectId, worktreeId, index, dropAfter });
    await get().refreshStatus(projectId, worktreeId);
    await get().refreshStashes(projectId, worktreeId);
  },

  dropStash: async (projectId, worktreeId, index) => {
    await gitStashDrop({ projectId, worktreeId, index });
    await get().refreshStashes(projectId, worktreeId);
  },

  refreshTags: async (projectId, worktreeId) => {
    try {
      const list = await gitTagList({ projectId, worktreeId });
      set((state) => ({ tags: { ...state.tags, [worktreeId]: list } }));
    } catch (e) {
      console.warn(e);
    }
  },

  createTag: async (projectId, worktreeId, name, target, message) => {
    await gitTagCreate({
      projectId,
      worktreeId,
      name,
      ...(target !== undefined ? { target } : {}),
      ...(message !== undefined ? { message } : {}),
    });
    await get().refreshTags(projectId, worktreeId);
  },

  deleteTag: async (projectId, worktreeId, name) => {
    await gitTagDelete({ projectId, worktreeId, name });
    await get().refreshTags(projectId, worktreeId);
  },

  cherryPick: async (projectId, worktreeId, oid) => {
    const result = await gitCherryPick({ projectId, worktreeId, oid });
    await get().refreshStatus(projectId, worktreeId);
    return result.oid;
  },

  revertCommit: async (projectId, worktreeId, oid) => {
    const result = await gitRevert({ projectId, worktreeId, oid });
    await get().refreshStatus(projectId, worktreeId);
    return result.oid;
  },

  applyHunk: async (projectId, worktreeId, patch, reverse) => {
    await gitApplyPatch({
      projectId,
      worktreeId,
      patch,
      reverse,
      cached: true,
    });
    await get().refreshStatus(projectId, worktreeId);
  },

  generateCommitMessage: async (projectId, worktreeId) => {
    set((state) => ({
      generatingCommitMsg: {
        ...state.generatingCommitMsg,
        [worktreeId]: true,
      },
      commitError: { ...state.commitError, [worktreeId]: null },
    }));
    try {
      // The backend reads the *staged* diff (falling back to the full
      // working-tree diff). Mirror the commit flow: if nothing is staged,
      // stage everything first so the panel reflects what will be committed.
      // Refresh first so the staged/unstaged decision isn't made off a stale
      // snapshot — that mismatch is what produced spurious "nothing staged".
      await get().refreshStatus(projectId, worktreeId);
      const status = get().status[worktreeId] ?? null;
      const hasStaged =
        status?.entries.some((e) => e.bucket === "staged") ?? false;
      if (!hasStaged) {
        const paths = stageablePaths(status);
        if (paths.length > 0) {
          await gitStage({ projectId, worktreeId, paths });
          await get().refreshStatus(projectId, worktreeId);
        }
      }
      const { message } = await gitGenerateCommitMessage({
        projectId,
        worktreeId,
      });
      set((state) => ({
        commitMessage: { ...state.commitMessage, [worktreeId]: message },
        generatingCommitMsg: {
          ...state.generatingCommitMsg,
          [worktreeId]: false,
        },
      }));
    } catch (e) {
      set((state) => ({
        generatingCommitMsg: {
          ...state.generatingCommitMsg,
          [worktreeId]: false,
        },
        commitError: { ...state.commitError, [worktreeId]: errMsg(e) },
      }));
    }
  },
}));

function setRemote(
  set: (updater: (state: GitState) => Partial<GitState>) => void,
  worktreeId: string,
  next: RemoteState,
): void {
  set((state) => ({
    remote: { ...state.remote, [worktreeId]: next },
  }));
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Paths to stage for a "commit everything" sweep: every unstaged tracked
 * change plus every untracked file. Conflicted entries are excluded — they
 * need resolution before they can be staged.
 */
function stageablePaths(status: StatusReport | null): string[] {
  if (!status) return [];
  const paths = new Set<string>();
  for (const e of status.entries) {
    if (e.bucket === "unstaged" || e.bucket === "untracked") {
      paths.add(e.path);
    }
  }
  return [...paths];
}

/** Bucket -> ordered list (matches the GitPanel section order). */
export function partitionByBucket(entries: StatusEntry[]) {
  const out = {
    staged: [] as StatusEntry[],
    unstaged: [] as StatusEntry[],
    untracked: [] as StatusEntry[],
    conflicted: [] as StatusEntry[],
  };
  for (const e of entries) {
    out[e.bucket].push(e);
  }
  return out;
}
