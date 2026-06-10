import { actionList } from "~/ipc/actions.ts";
import { envDotenvRenderForWorktree } from "~/ipc/env.ts";
import { useTerminalDockStore } from "~/stores/terminalDockStore.ts";

/**
 * Things to do right after a new worktree is created:
 *
 * 1. Render `.env.local` from `.env` + `.oxyris/.env.template` (no-op when
 *    the template doesn't exist). Always fires — doesn't need a session.
 * 2. Run every action in the project with `auto_run_on_worktree_create=true`
 *    in its own terminal tab. Skipped silently when no session is active
 *    (PTYs are session-scoped).
 */
export async function runAutoActionsOnWorktreeCreate(input: {
  projectId: string;
  worktreeId: string;
  sessionId: string | null;
  onBeforeRun?: () => void;
}): Promise<void> {
  // (1) dotenv render — fire first, no session needed.
  try {
    await envDotenvRenderForWorktree({ worktree_id: input.worktreeId });
  } catch {
    /* surfaces in the env chip later if it matters */
  }

  // (2) auto-run actions — needs a session.
  if (!input.sessionId) return;
  let actions;
  try {
    actions = await actionList({ project_id: input.projectId });
  } catch {
    return;
  }
  const autoRun = actions.filter((a) => a.auto_run_on_worktree_create);
  if (autoRun.length === 0) return;
  input.onBeforeRun?.();
  // Queue each command for the dock to spawn — it owns the PTY tabs. Requests
  // wait in the queue until a dock for this session mounts, then drain in order.
  for (const action of autoRun) {
    useTerminalDockStore.getState().enqueue(input.sessionId, action.command);
  }
}
