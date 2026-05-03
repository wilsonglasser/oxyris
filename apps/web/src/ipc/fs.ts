import { invoke } from "@tauri-apps/api/core";

export type FsEntry = {
  name: string;
  is_dir: boolean;
  is_symlink: boolean;
  size: number | null;
  modified_secs: number | null;
};

export type FsListDirOutput = {
  abs_path: string;
  entries: FsEntry[];
};

export type FsReadFileOutput = {
  abs_path: string;
  content: string;
  bytes_read: number;
  truncated: boolean;
};

export type FsReadFileBytesOutput = {
  abs_path: string;
  bytes_b64: string;
  mime: string;
  bytes_read: number;
  truncated: boolean;
};

export type FsWriteFileOutput = {
  abs_path: string;
  bytes_written: number;
};

export type FsOpenExternalOutput = {
  editor: string;
  command: string;
};

export type ExternalEditorInfo = {
  id: string;
  label: string;
  available: boolean;
};

export function fsListDir(args: {
  projectId: string;
  worktreeId: string;
  relPath?: string;
  showHidden?: boolean;
}): Promise<FsListDirOutput> {
  return invoke<FsListDirOutput>("fs_list_dir", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      rel_path: args.relPath ?? "",
      show_hidden: args.showHidden ?? false,
    },
  });
}

export function fsReadFile(args: {
  projectId: string;
  worktreeId: string;
  relPath: string;
  maxBytes?: number;
}): Promise<FsReadFileOutput> {
  return invoke<FsReadFileOutput>("fs_read_file", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      rel_path: args.relPath,
      max_bytes: args.maxBytes,
    },
  });
}

export function fsWriteFile(args: {
  projectId: string;
  worktreeId: string;
  relPath: string;
  content: string;
}): Promise<FsWriteFileOutput> {
  return invoke<FsWriteFileOutput>("fs_write_file", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      rel_path: args.relPath,
      content: args.content,
    },
  });
}

export function fsOpenExternal(args: {
  projectId: string;
  worktreeId: string;
  relPath: string;
  editor?: string;
}): Promise<FsOpenExternalOutput> {
  return invoke<FsOpenExternalOutput>("fs_open_external", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      rel_path: args.relPath,
      editor: args.editor,
    },
  });
}

export function fsExternalEditors(): Promise<ExternalEditorInfo[]> {
  return invoke<ExternalEditorInfo[]>("fs_external_editors");
}

export function fsReadFileBytes(args: {
  projectId: string;
  worktreeId: string;
  relPath: string;
  maxBytes?: number;
}): Promise<FsReadFileBytesOutput> {
  return invoke<FsReadFileBytesOutput>("fs_read_file_bytes", {
    input: {
      project_id: args.projectId,
      worktree_id: args.worktreeId,
      rel_path: args.relPath,
      max_bytes: args.maxBytes,
    },
  });
}

const IMG_EXT = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "svg",
  "bmp",
  "ico",
]);

export type PreviewKind = "markdown" | "image" | "pdf" | "text";

export function previewKindFor(relPath: string): PreviewKind {
  const ext = relPath.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "md" || ext === "markdown") return "markdown";
  if (ext === "pdf") return "pdf";
  if (IMG_EXT.has(ext)) return "image";
  return "text";
}
