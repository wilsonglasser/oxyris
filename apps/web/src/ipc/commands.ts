import { invoke } from "@tauri-apps/api/core";

// Typed wrappers around Tauri commands. One file per surface keeps drift
// between the frontend and the Rust handlers in `apps/desktop/src/tauri_commands/`
// easy to spot.

// ────── Sprint 1: hello-world roundtrip ─────────────────────────────────────

export async function greet(name: string): Promise<string> {
  return invoke<string>("greet", { name });
}

// ────── Sprint 2: Project aggregate ─────────────────────────────────────────

export type Environment =
  | { kind: "windows" }
  | { kind: "wsl"; distro: string };

export type ProjectRow = {
  id: string;
  name: string;
  environment: Environment;
  root_path: string;
  session_count: number;
  created_at: string;
  last_activity_at: string;
};

export type ProjectError =
  | { code: "domain"; message: string }
  | { code: "concurrency" }
  | { code: "storage"; message: string }
  | { code: "projection"; message: string };

/**
 * Tauri rejects commands by returning the serialized error. We surface it as
 * a typed {@link ProjectError} so callers can render via i18n without string-
 * sniffing.
 */
export class ProjectCommandError extends Error {
  readonly tauri: ProjectError;
  constructor(tauri: ProjectError) {
    super(
      tauri.code === "concurrency"
        ? "concurrency"
        : `${tauri.code}: ${tauri.message}`,
    );
    this.tauri = tauri;
    this.name = "ProjectCommandError";
  }
}

function wrapError(unknown: unknown): never {
  if (
    unknown &&
    typeof unknown === "object" &&
    "code" in unknown &&
    typeof (unknown as { code: unknown }).code === "string"
  ) {
    throw new ProjectCommandError(unknown as ProjectError);
  }
  throw unknown;
}

export async function projectCreate(input: {
  name: string;
  environment: Environment;
  root_path: string;
}): Promise<{ id: string }> {
  try {
    return await invoke<{ id: string }>("project_create", { input });
  } catch (err) {
    wrapError(err);
  }
}

export async function projectRename(input: {
  id: string;
  new_name: string;
}): Promise<void> {
  try {
    await invoke<void>("project_rename", { input });
  } catch (err) {
    wrapError(err);
  }
}

export async function projectDelete(input: { id: string }): Promise<void> {
  try {
    await invoke<void>("project_delete", { input });
  } catch (err) {
    wrapError(err);
  }
}

export async function projectList(): Promise<ProjectRow[]> {
  return invoke<ProjectRow[]>("project_list");
}

export type PathValidation = {
  exists: boolean;
  is_dir: boolean;
  is_git_repo: boolean;
  warning: string | null;
};

export async function projectValidatePath(input: {
  environment: Environment;
  path: string;
}): Promise<PathValidation> {
  return invoke<PathValidation>("project_validate_path", { input });
}
