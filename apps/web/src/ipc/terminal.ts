import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type TerminalKind = "shell" | "claude";

export type TerminalInfo = {
  id: string;
  session_id: string;
  title: string;
  cwd: string;
  /** "claude" = the pure-mode TUI PTY (owned by its own pane, hidden in the dock). */
  kind: TerminalKind;
};

export async function terminalSpawn(input: {
  session_id: string;
  cols: number;
  rows: number;
}): Promise<TerminalInfo> {
  return invoke("terminal_spawn", { input });
}

/**
 * Spawn the interactive `claude` TUI in a PTY for a Pure-mode session
 * ("Claude Code puro"). Same event/attach plumbing as a regular terminal —
 * use {@link onTerminalOutput}/{@link terminalAttach}/{@link terminalWrite}
 * with the returned id.
 */
export async function claudePtySpawn(input: {
  session_id: string;
  cols: number;
  rows: number;
}): Promise<TerminalInfo> {
  return invoke("claude_pty_spawn", { input });
}

export async function terminalWrite(input: {
  id: string;
  data: string;
}): Promise<void> {
  await invoke("terminal_write", { input });
}

/**
 * Best-effort auto-title for a pure-mode session, read from claude's own
 * transcript. Returns the applied title, or `null` when the session is already
 * titled or no transcript/title is available yet. Safe to call repeatedly.
 */
export async function claudePureRefreshTitle(input: {
  session_id: string;
}): Promise<string | null> {
  return invoke("claude_pure_refresh_title", { input });
}

export async function terminalResize(input: {
  id: string;
  cols: number;
  rows: number;
}): Promise<void> {
  await invoke("terminal_resize", { input });
}

export async function terminalKill(input: { id: string }): Promise<void> {
  await invoke("terminal_kill", { input });
}

export async function terminalList(input: {
  session_id: string;
}): Promise<TerminalInfo[]> {
  return invoke("terminal_list", { input });
}

export async function terminalRename(input: {
  id: string;
  title: string;
}): Promise<void> {
  await invoke("terminal_rename", { input });
}

export type TerminalAttachSnapshot = {
  data: string;
  last_seq: number;
};

/**
 * Returns whatever the backend reader has emitted so far on this terminal,
 * tagged with the highest sequence number it carries. Pair with
 * {@link onTerminalOutput} (which ships `seq`) to drop replayed bytes from
 * the live stream — eliminates the attach-vs-emit race when the user just
 * spawned a new tab.
 */
export async function terminalAttach(input: {
  id: string;
}): Promise<TerminalAttachSnapshot> {
  return invoke("terminal_attach", { input });
}

export async function onTerminalOutput(
  id: string,
  cb: (seq: number, chunk: string) => void,
): Promise<UnlistenFn> {
  return listen<{ seq: number; data: string }>(
    `terminal:${id}:output`,
    (e) => cb(e.payload.seq, e.payload.data),
  );
}

export async function onTerminalExit(
  id: string,
  cb: (reason: string) => void,
): Promise<UnlistenFn> {
  return listen<string>(`terminal:${id}:exit`, (e) => cb(e.payload));
}
