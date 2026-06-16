import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Matches `SupervisorKind` (serde snake_case) in `oxyris-supervisor`. */
export type SupervisorKind = "multi_model" | "claude";

export type AutopilotEngageInput = {
  session_id: string;
  mission: string;
  supervisor: SupervisorKind;
  model?: string;
  /** OpenAI-compatible base URL — required for the multi-model supervisor. */
  base_url?: string;
  api_key?: string;
  max_turns?: number;
};

export async function autopilotEngage(
  input: AutopilotEngageInput,
): Promise<void> {
  await invoke("autopilot_engage", { input });
}

export async function autopilotDisengage(sessionId: string): Promise<void> {
  await invoke("autopilot_disengage", { input: { session_id: sessionId } });
}

/**
 * App-wide default supervisor config, persisted backend-side so an MCP-driven
 * engage (Claude handing off via `oxyris_autopilot_engage`) can build a config
 * without the frontend in the loop. Mirrors `AutopilotDefaults` in
 * `infra/autopilot_config.rs` (snake_case wire shape).
 */
export type AutopilotDefaults = {
  supervisor: SupervisorKind;
  model: string;
  base_url: string;
  api_key: string;
  claude_model: string;
  max_turns: number | null;
};

export async function autopilotGetDefaults(): Promise<AutopilotDefaults> {
  return invoke<AutopilotDefaults>("autopilot_get_defaults");
}

export async function autopilotSetDefaults(
  input: AutopilotDefaults,
): Promise<void> {
  await invoke("autopilot_set_defaults", { input });
}

/** Decision/outcome the backend pilot emits, mirroring `AutopilotEvent`. */
export type AutopilotEvent =
  | { kind: "thinking" }
  | { kind: "reasoning"; text: string }
  | { kind: "approved" }
  | { kind: "rejected"; reason: string }
  | { kind: "replied"; text: string }
  | { kind: "done"; summary: string }
  | { kind: "halted"; reason: string }
  | { kind: "escalated"; why: string }
  | { kind: "error"; message: string };

/** Subscribe to a session's auto-pilot decision stream (for the mini-log). */
export async function onAutopilotEvent(
  sessionId: string,
  cb: (event: AutopilotEvent) => void,
): Promise<UnlistenFn> {
  return listen<AutopilotEvent>(
    `session:${sessionId}:autopilot`,
    (e) => cb(e.payload),
  );
}
