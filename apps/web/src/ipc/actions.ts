import { invoke } from "@tauri-apps/api/core";

export type ActionRow = {
  id: string;
  project_id: string;
  name: string;
  command: string;
  keybinding: string | null;
  auto_run_on_worktree_create: boolean;
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
}): Promise<ActionRow> {
  return invoke("action_upsert", { input });
}

export async function actionDelete(input: { id: string }): Promise<void> {
  await invoke("action_delete", { input });
}
