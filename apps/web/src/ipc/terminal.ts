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
  /**
   * Extra `--append-system-prompt` text for the claude TUI — used to pin the
   * response language. Merged with the MCP tool nudge backend-side.
   */
  system_prompt?: string;
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

export type PureState = {
  /** A prompt/menu is on screen — the red "wants input" dot. */
  needs_input: boolean;
  /** A turn is in flight with no prompt waiting — the blue "busy" dot. */
  busy: boolean;
};

/**
 * Ground-truth pure-turn dot state for a session, read off the backend's live
 * sniffer + output clock. Used to seed the dot on attach, before the first
 * {@link onPureState} snapshot lands (the per-session listener may register
 * after the backend already emitted the current state).
 */
export async function claudePureState(input: {
  session_id: string;
}): Promise<PureState> {
  return invoke("claude_pure_state", { input });
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

/**
 * Subscribe to a pure session's backend turn-state snapshots, keyed by the
 * session id (not the terminal id — the backend emits per session so background
 * watchers can listen without first resolving the claude PTY).
 *
 * Unlike the old edge `pure-signal`, this is a LEVEL snapshot: the backend emits
 * the full {@link PureState} whenever it changes AND on a heartbeat, so a
 * snapshot missed while no listener was attached self-heals on the next tick.
 * The consumer derives chime/attention transitions by diffing against the
 * previous snapshot — see the single bridge in `Sidebar`. Driving this from the
 * backend (not a frontend sniffer + `setTimeout`) is what makes detection
 * survive the window losing focus (the WebView throttles background timers).
 */
export async function onPureState(
  sessionId: string,
  cb: (state: PureState) => void,
): Promise<UnlistenFn> {
  return listen<PureState>(
    `session:${sessionId}:pure-state`,
    (e) => cb(e.payload),
  );
}

export type TakeoverState = {
  /** True while a phone holds this session's pure PTY; the desktop view freezes. */
  active: boolean;
  /** Who took over — `"mobile"` today. */
  by: string;
};

/**
 * Subscribe to mobile-takeover transitions for a pure session. When `active`,
 * the desktop terminal must stop sending input/resize (the phone owns the PTY)
 * and show a frozen overlay; on release it resumes. Keyed by session id to match
 * the backend's `session:<id>:takeover` emit.
 */
export async function onTakeover(
  sessionId: string,
  cb: (state: TakeoverState) => void,
): Promise<UnlistenFn> {
  return listen<TakeoverState>(
    `session:${sessionId}:takeover`,
    (e) => cb(e.payload),
  );
}
