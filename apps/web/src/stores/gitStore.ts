import { create } from "zustand";
import {
  gitBranchCreate,
  gitBranchDelete,
  gitCheckout,
  gitCommit,
  gitDiffFile,
  gitFetch,
  gitGenerateCommitMessage,
  gitLog,
  gitPull,
  gitPush,
  gitStage,
  gitStatus,
  gitUnstage,
  type CommitInfo,
  type DiffMode,
  type FileDiff,
  type StatusEntry,
  type StatusReport,
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

  generateCommitMessage: async (projectId, worktreeId) => {
    set((state) => ({
      generatingCommitMsg: {
        ...state.generatingCommitMsg,
        [worktreeId]: true,
      },
      commitError: { ...state.commitError, [worktreeId]: null },
    }));
    try {
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
