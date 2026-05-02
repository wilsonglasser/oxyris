import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Environment } from "~/ipc/commands.ts";

// ────── types ──────────────────────────────────────────────────────────────

export type RuntimeMode = "supervised" | "accept_edits" | "full_access" | "plan";
export type ThinkingMode = "auto" | "off" | "on";
export type EnvMode = "default" | "worktree";

export type AssistantBlock =
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool_use"; id: string; name: string; input: unknown }
  | {
      kind: "tool_result";
      tool_use_id: string;
      output: unknown;
      is_error: boolean;
    };

export type TurnStatus =
  | "streaming"
  | "completed"
  | "failed"
  | "interrupted";

export type TurnEntry = {
  id: string;
  user_text: string;
  blocks: AssistantBlock[];
  status: TurnStatus;
  started_at: string;
  completed_at: string | null;
  total_cost_usd: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  error_message: string | null;
};

export type SessionStatus = "idle" | "running" | "stopped" | "errored";

export type SessionSnapshot = {
  id: string;
  project_id: string;
  worktree_id: string | null;
  provider_id: string;
  model: string;
  thinking: ThinkingMode;
  runtime: RuntimeMode;
  status: SessionStatus;
  turns: TurnEntry[];
  created_at: string;
  provider_session_id: string | null;
  title: string | null;
  env_mode: EnvMode;
};

export type SessionSummary = {
  id: string;
  project_id: string;
  worktree_id: string | null;
  provider_id: string;
  model: string;
  status: string;
  turn_count: number;
  created_at: string;
  last_activity_at: string;
  title: string | null;
};

// Persisted SessionEvent shapes (the backend emits these via
// `session:<id>:event`). Discriminated on `kind`.
export type SessionEventPayload =
  | {
      kind: "SessionStarted";
      id: string;
      project_id: string;
      worktree_id: string | null;
      provider_id: string;
      model: string;
      thinking: ThinkingMode;
      runtime: RuntimeMode;
      env_mode: EnvMode;
      created_at: string;
    }
  | { kind: "SessionEnvModeChanged"; mode: EnvMode }
  | { kind: "SessionStopped"; at: string }
  | { kind: "SessionErrored"; at: string; message: string }
  | {
      kind: "TurnStarted";
      turn_id: string;
      user_text: string;
      started_at: string;
    }
  | {
      kind: "TurnAssistantBlockAppended";
      turn_id: string;
      block: AssistantBlock;
    }
  | {
      kind: "TurnCompleted";
      turn_id: string;
      total_cost_usd: number | null;
      input_tokens: number | null;
      output_tokens: number | null;
      completed_at: string;
    }
  | {
      kind: "TurnFailed";
      turn_id: string;
      message: string;
      completed_at: string;
    }
  | { kind: "TurnInterrupted"; turn_id: string; at: string }
  | { kind: "ProviderSessionAttached"; provider_session_id: string }
  | { kind: "SessionResumed"; at: string }
  | { kind: "SessionRenamed"; title: string }
  | { kind: "SessionDeleted"; at: string };

export type EmittedSessionEvent = {
  session_id: string;
  version: number;
  event: SessionEventPayload;
};

// ────── commands ───────────────────────────────────────────────────────────

export async function sessionStart(input: {
  project_id: string;
  worktree_id?: string;
  provider_id: string;
  environment: Environment;
  cwd: string;
  model: string;
  thinking?: ThinkingMode;
  runtime?: RuntimeMode;
  system_prompt?: string;
  env_mode?: EnvMode;
}): Promise<{ session_id: string }> {
  return invoke("session_start", { input });
}

export async function sessionSendMessage(input: {
  session_id: string;
  text: string;
}): Promise<{ turn_id: string }> {
  return invoke("session_send_message", { input });
}

export async function sessionInterrupt(input: {
  session_id: string;
  turn_id: string;
}): Promise<void> {
  await invoke("session_interrupt", { input });
}

export async function sessionStop(input: {
  session_id: string;
}): Promise<void> {
  await invoke("session_stop", { input });
}

export async function sessionResume(input: {
  session_id: string;
}): Promise<void> {
  await invoke("session_resume", { input });
}

export async function sessionRename(input: {
  session_id: string;
  title: string;
}): Promise<void> {
  await invoke("session_rename", { input });
}

export async function sessionDelete(input: {
  session_id: string;
}): Promise<void> {
  await invoke("session_delete", { input });
}

export async function sessionList(input: {
  project_id: string;
}): Promise<SessionSummary[]> {
  return invoke("session_list", { input });
}

export async function sessionGet(input: {
  session_id: string;
}): Promise<SessionSnapshot | null> {
  return invoke("session_get", { input });
}

// ────── diff ───────────────────────────────────────────────────────────────

export type FileStatus =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "copied"
  | "typechange"
  | "unchanged";

export type FileDiff = {
  path: string;
  old_path: string | null;
  status: FileStatus;
  old_content: string | null;
  new_content: string | null;
  unified: string;
};

export type TurnDiff = {
  files: FileDiff[];
};

export async function sessionTurnDiff(input: {
  session_id: string;
  turn_id: string;
}): Promise<TurnDiff> {
  return invoke("session_turn_diff", { input });
}

export async function sessionTurnRevert(input: {
  session_id: string;
  turn_id: string;
}): Promise<void> {
  await invoke("session_turn_revert", { input });
}

/** Subscribe to streaming events for one session. Returns the unsubscribe fn. */
export async function onSessionEvent(
  sessionId: string,
  cb: (ev: EmittedSessionEvent) => void,
): Promise<UnlistenFn> {
  return listen<EmittedSessionEvent>(`session:${sessionId}:event`, (event) => {
    cb(event.payload);
  });
}
