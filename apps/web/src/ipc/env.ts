import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type EnvMode = "default" | "worktree";

export type EnvTemplate = {
  has_template: boolean;
  template_path: string | null;
  docker_project: string;
  port_offset: number;
};

export type EnvStatus = {
  up: boolean;
  services: string[];
};

export type DotenvRenderOutcome =
  | { kind: "generated"; path: string; byte_count: number }
  | { kind: "no_template" }
  | { kind: "manual_override"; path: string };

export type DotenvStatus = {
  has_template: boolean;
  has_local: boolean;
  stale: boolean;
  manual_override: boolean;
  template_path: string | null;
  local_path: string | null;
  template_modified_secs: number | null;
  local_modified_secs: number | null;
};

export async function envDotenvRenderForWorktree(input: {
  worktree_id: string;
}): Promise<DotenvRenderOutcome> {
  return invoke("env_dotenv_render_for_worktree", { input });
}

export async function envDotenvStatusForWorktree(input: {
  worktree_id: string;
}): Promise<DotenvStatus> {
  return invoke("env_dotenv_status_for_worktree", { input });
}

export type DockerCleanupReport = {
  orphan_projects: string[];
  containers_removed: number;
  volumes_removed: number;
  networks_removed: number;
};

/** Subscribe to the boot-time docker cleanup report (only fires when
 * something was actually pruned). Returns the unsubscribe function. */
export async function onDockerCleanup(
  cb: (report: DockerCleanupReport) => void,
): Promise<UnlistenFn> {
  return listen<DockerCleanupReport>("docker:cleanup", (e) => cb(e.payload));
}

export async function envTemplateForWorktree(input: {
  worktree_id: string;
}): Promise<EnvTemplate> {
  return invoke("env_template_for_worktree", { input });
}

export async function envStatusForWorktree(input: {
  worktree_id: string;
}): Promise<EnvStatus> {
  return invoke("env_status_for_worktree", { input });
}

export async function envUpForWorktree(input: {
  worktree_id: string;
  session_id: string;
}): Promise<{ id: string }> {
  return invoke("env_up_for_worktree", { input });
}

export async function envDownForWorktree(input: {
  worktree_id: string;
  session_id: string;
}): Promise<{ id: string }> {
  return invoke("env_down_for_worktree", { input });
}

export async function sessionSetEnvMode(input: {
  session_id: string;
  mode: EnvMode;
}): Promise<void> {
  return invoke("session_set_env_mode", { input });
}

/** Subscribe to env-mode changes for one session (cross-window sync). */
export async function onSessionEnvModeChanged(
  sessionId: string,
  cb: (mode: EnvMode) => void,
): Promise<UnlistenFn> {
  return listen<{ mode: EnvMode }>(`session:${sessionId}:env_mode`, (e) =>
    cb(e.payload.mode),
  );
}
