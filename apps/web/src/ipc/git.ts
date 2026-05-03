import { invoke } from "@tauri-apps/api/core";

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
  return invoke<CommitResult>("git_commit", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      message: args.message,
      amend: args.amend ?? false,
    },
  });
}

export function gitFetch(args: {
  projectId: string;
  worktreeId: string;
  remote?: string;
}): Promise<RemoteOpResult> {
  return invoke<RemoteOpResult>("git_fetch", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      remote: args.remote,
    },
  });
}

export function gitPull(args: {
  projectId: string;
  worktreeId: string;
  remote?: string;
  branch?: string;
  rebase?: boolean;
}): Promise<RemoteOpResult> {
  return invoke<RemoteOpResult>("git_pull", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      remote: args.remote,
      branch: args.branch,
      rebase: args.rebase ?? false,
    },
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
  return invoke<RemoteOpResult>("git_push", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      remote: args.remote,
      branch: args.branch,
      force: args.force ?? false,
      set_upstream: args.setUpstream ?? false,
    },
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
  return invoke<{ message: string }>("git_generate_commit_message", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
    },
  });
}
