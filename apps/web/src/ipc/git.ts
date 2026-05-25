import { invoke } from "@tauri-apps/api/core";

export type GitErrorPayload = { code: string; message: string };

/**
 * Tauri rejects git commands with the serialized `TauriGitError`
 * (`apps/desktop/src/tauri_commands/git.rs`) — a plain `{ code, message }`
 * object. Wrapping it as an `Error` keeps callers from rendering
 * `[object Object]` when they `String(e)` the rejection.
 */
export class GitCommandError extends Error {
  readonly code: string;
  constructor(payload: GitErrorPayload) {
    super(payload.message || payload.code);
    this.code = payload.code;
    this.name = "GitCommandError";
  }
}

async function invokeGit<T>(cmd: string, input: unknown): Promise<T> {
  try {
    return await invoke<T>(cmd, { input });
  } catch (err) {
    if (
      err &&
      typeof err === "object" &&
      "message" in err &&
      typeof (err as { message: unknown }).message === "string"
    ) {
      throw new GitCommandError(err as GitErrorPayload);
    }
    throw err;
  }
}

export type FileStatus =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "copied"
  | "typechange"
  | "unchanged";

export type StatusBucket = "staged" | "unstaged" | "untracked" | "conflicted";

export type DiffMode =
  | "working_vs_head"
  | "staged_vs_head"
  | "working_vs_staged";

export type StatusEntry = {
  path: string;
  old_path: string | null;
  bucket: StatusBucket;
  status: FileStatus;
};

export type AheadBehind = {
  ahead: number;
  behind: number;
};

export type StatusReport = {
  entries: StatusEntry[];
  branch: string | null;
  ahead_behind: AheadBehind | null;
};

export type FileDiff = {
  path: string;
  old_path: string | null;
  status: FileStatus;
  old_content: string | null;
  new_content: string | null;
  unified: string;
};

export type CommitResult = {
  oid: string;
  message: string;
  branch: string | null;
};

export type RemoteOpResult = {
  stdout: string;
  stderr: string;
};

export type CommitInfo = {
  oid: string;
  short_oid: string;
  summary: string;
  message: string;
  author_name: string;
  author_email: string;
  author_time: number;
  parents: string[];
};

export type ConflictContents = {
  path: string;
  base: string | null;
  ours: string | null;
  theirs: string | null;
  workdir: string | null;
};

export function gitStatus(args: {
  projectId: string;
  worktreeId: string;
}): Promise<StatusReport> {
  return invoke<StatusReport>("git_status", {
    input: { project_id: args.projectId, worktree_id: args.worktreeId },
  });
}

export function gitDiffFile(args: {
  projectId: string;
  worktreeId: string;
  path: string;
  mode: DiffMode;
}): Promise<FileDiff> {
  return invoke<FileDiff>("git_diff_file", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      path: args.path,
      mode: args.mode,
    },
  });
}

export function gitStage(args: {
  projectId: string;
  worktreeId: string;
  paths: string[];
}): Promise<void> {
  return invoke<void>("git_stage", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      paths: args.paths,
    },
  });
}

export function gitUnstage(args: {
  projectId: string;
  worktreeId: string;
  paths: string[];
}): Promise<void> {
  return invoke<void>("git_unstage", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      paths: args.paths,
    },
  });
}

export function gitCommit(args: {
  projectId: string;
  worktreeId: string;
  message: string;
  amend?: boolean;
}): Promise<CommitResult> {
  return invokeGit<CommitResult>("git_commit", {
    project_id: args.projectId,
    worktree_id: args.worktreeId,
    message: args.message,
    amend: args.amend ?? false,
  });
}

export function gitFetch(args: {
  projectId: string;
  worktreeId: string;
  remote?: string;
}): Promise<RemoteOpResult> {
  return invokeGit<RemoteOpResult>("git_fetch", {
    project_id: args.projectId,
    worktree_id: args.worktreeId,
    remote: args.remote,
  });
}

export function gitPull(args: {
  projectId: string;
  worktreeId: string;
  remote?: string;
  branch?: string;
  rebase?: boolean;
}): Promise<RemoteOpResult> {
  return invokeGit<RemoteOpResult>("git_pull", {
    project_id: args.projectId,
    worktree_id: args.worktreeId,
    remote: args.remote,
    branch: args.branch,
    rebase: args.rebase ?? false,
  });
}

export function gitPush(args: {
  projectId: string;
  worktreeId: string;
  remote?: string;
  branch?: string;
  force?: boolean;
  setUpstream?: boolean;
}): Promise<RemoteOpResult> {
  return invokeGit<RemoteOpResult>("git_push", {
    project_id: args.projectId,
    worktree_id: args.worktreeId,
    remote: args.remote,
    branch: args.branch,
    force: args.force ?? false,
    set_upstream: args.setUpstream ?? false,
  });
}

export function gitCheckout(args: {
  projectId: string;
  worktreeId: string;
  name: string;
}): Promise<void> {
  return invoke<void>("git_checkout", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      name: args.name,
    },
  });
}

export function gitBranchCreate(args: {
  projectId: string;
  worktreeId: string;
  name: string;
  from?: string;
  checkout?: boolean;
}): Promise<void> {
  return invoke<void>("git_branch_create", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      name: args.name,
      from: args.from,
      checkout: args.checkout ?? false,
    },
  });
}

export function gitBranchDelete(args: {
  projectId: string;
  worktreeId: string;
  name: string;
}): Promise<void> {
  return invoke<void>("git_branch_delete", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      name: args.name,
    },
  });
}

export function gitLog(args: {
  projectId: string;
  worktreeId: string;
  limit?: number;
  rev?: string;
}): Promise<CommitInfo[]> {
  return invoke<CommitInfo[]>("git_log", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      limit: args.limit ?? 50,
      rev: args.rev,
    },
  });
}

export function gitGetConflict(args: {
  projectId: string;
  worktreeId: string;
  path: string;
}): Promise<ConflictContents> {
  return invoke<ConflictContents>("git_get_conflict", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      path: args.path,
    },
  });
}

export function gitResolve(args: {
  projectId: string;
  worktreeId: string;
  path: string;
  content: string;
}): Promise<void> {
  return invoke<void>("git_resolve", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      path: args.path,
      content: args.content,
    },
  });
}

export type StashEntry = {
  index: number;
  short_id: string;
  oid: string;
  message: string;
  time: number;
};

export type TagInfo = {
  name: string;
  oid: string;
  message: string | null;
  annotated: boolean;
};

export function gitStashList(args: {
  projectId: string;
  worktreeId: string;
}): Promise<StashEntry[]> {
  return invoke<StashEntry[]>("git_stash_list", {
    input: { project_id: args.projectId, worktree_id: args.worktreeId },
  });
}

export function gitStashSave(args: {
  projectId: string;
  worktreeId: string;
  message: string;
  includeUntracked?: boolean;
}): Promise<string> {
  return invoke<string>("git_stash_save", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      message: args.message,
      include_untracked: args.includeUntracked ?? false,
    },
  });
}

export function gitStashApply(args: {
  projectId: string;
  worktreeId: string;
  index: number;
  dropAfter?: boolean;
}): Promise<void> {
  return invoke<void>("git_stash_apply", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      index: args.index,
      drop_after: args.dropAfter ?? false,
    },
  });
}

export function gitStashDrop(args: {
  projectId: string;
  worktreeId: string;
  index: number;
}): Promise<void> {
  return invoke<void>("git_stash_drop", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      index: args.index,
    },
  });
}

export function gitTagList(args: {
  projectId: string;
  worktreeId: string;
}): Promise<TagInfo[]> {
  return invoke<TagInfo[]>("git_tag_list", {
    input: { project_id: args.projectId, worktree_id: args.worktreeId },
  });
}

export function gitTagCreate(args: {
  projectId: string;
  worktreeId: string;
  name: string;
  target?: string;
  message?: string;
  force?: boolean;
}): Promise<void> {
  return invoke<void>("git_tag_create", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      name: args.name,
      target: args.target,
      message: args.message,
      force: args.force ?? false,
    },
  });
}

export function gitTagDelete(args: {
  projectId: string;
  worktreeId: string;
  name: string;
}): Promise<void> {
  return invoke<void>("git_tag_delete", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      name: args.name,
    },
  });
}

export function gitCherryPick(args: {
  projectId: string;
  worktreeId: string;
  oid: string;
}): Promise<{ oid: string | null }> {
  return invoke<{ oid: string | null }>("git_cherry_pick", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      oid: args.oid,
    },
  });
}

export function gitDiffRevs(args: {
  projectId: string;
  worktreeId: string;
  from: string;
  to: string;
  findRenames?: boolean;
}): Promise<FileDiff[]> {
  return invoke<FileDiff[]>("git_diff_revs", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      from: args.from,
      to: args.to,
      find_renames: args.findRenames ?? true,
    },
  });
}

export function gitRevert(args: {
  projectId: string;
  worktreeId: string;
  oid: string;
}): Promise<{ oid: string | null }> {
  return invoke<{ oid: string | null }>("git_revert", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      oid: args.oid,
    },
  });
}

export function gitApplyPatch(args: {
  projectId: string;
  worktreeId: string;
  patch: string;
  reverse?: boolean;
  cached?: boolean;
}): Promise<void> {
  return invoke<void>("git_apply_patch", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      patch: args.patch,
      reverse: args.reverse ?? false,
      cached: args.cached ?? true,
    },
  });
}

export function gitGenerateCommitMessage(args: {
  projectId: string;
  worktreeId: string;
}): Promise<{ message: string }> {
  return invokeGit<{ message: string }>("git_generate_commit_message", {
    project_id: args.projectId,
    worktree_id: args.worktreeId,
  });
}
