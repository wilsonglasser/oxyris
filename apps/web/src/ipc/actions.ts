import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ActionKind = "terminal_command" | "one_shot" | "github_workflow";

export type ActionRow = {
  id: string;
  project_id: string;
  name: string;
  command: string;
  keybinding: string | null;
  auto_run_on_worktree_create: boolean;
  icon: string;
  kind: ActionKind;
  show_in_sidebar: boolean;
  created_at: string;
  updated_at: string;
};

export async function actionList(input: {
  project_id: string;
}): Promise<ActionRow[]> {
  return invoke("action_list", { input });
}

export async function actionUpsert(input: {
  id?: string | null;
  project_id: string;
  name: string;
  command: string;
  keybinding?: string | null;
  auto_run_on_worktree_create: boolean;
  icon: string;
  kind: ActionKind;
  show_in_sidebar?: boolean;
}): Promise<ActionRow> {
  return invoke("action_upsert", { input });
}

export async function actionDelete(input: { id: string }): Promise<void> {
  await invoke("action_delete", { input });
}

export type ActionRunOutput = { run_id: string };

export async function actionRun(input: {
  action_id: string;
  project_id: string;
  worktree_id?: string | null;
}): Promise<ActionRunOutput> {
  return invoke("action_run", { input });
}

export type ActionStreamLine =
  | { kind: "stdout"; text: string }
  | { kind: "stderr"; text: string }
  | { kind: "exit"; code: number; success: boolean }
  | { kind: "error"; message: string };

export async function listenActionOutput(
  runId: string,
  cb: (line: ActionStreamLine) => void,
): Promise<UnlistenFn> {
  return await listen<ActionStreamLine>(`action:output:${runId}`, (e) =>
    cb(e.payload),
  );
}
