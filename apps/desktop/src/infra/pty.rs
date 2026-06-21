//! PTY supervisor — owns one ConPTY per terminal id, streams output to the
//! UI as Tauri events, and accepts user input + resize commands.
//!
//! Windows-only in this MVP slice (`Environment::Wsl` returns "not yet
//! supported" until the agent gains a `pty.spawn` op).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use oxyris_core::{AggregateId, Environment};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use regex::Regex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

/// Fan-out capacity for the mobile-takeover output broadcast. Sized to cover a
/// burst (e.g. `cargo run` flooding output) without a slow mobile reader
/// lagging out — on `Lagged` the mobile WS handler re-attaches a fresh snapshot
/// to resync, so an occasional drop is recoverable, just wasteful.
const OUTPUT_BROADCAST_CAP: usize = 4096;

use crate::infra::pure_signals::{
    PureSignal, PureSniffer, has_content_stripped, strip_ansi, strip_for_detection,
};

/// In-process notification of a pure-signal marker edge, pushed from the PTY
/// reader to the auto-pilot controller. (The frontend dot rides the separate
/// `session:<id>:pure-state` level snapshot, not these edges.) Lets the backend
/// react with the window unfocused — the whole point of moving detection off the
/// WebView.
#[derive(Debug, Clone)]
pub struct PureSignalNotice {
    pub session_id: AggregateId,
    pub terminal_id: String,
    pub signal: PureSignal,
}

/// Cap on the per-terminal replay buffer. This is the source of truth for how
/// much scrollback survives a re-attach (tab switch / remount): on attach the
/// frontend rebuilds a fresh xterm and replays this snapshot, so anything older
/// is gone for good. Sized to cover the frontend's `scrollback` cap (~50k lines)
/// — 256 KB was too small and silently truncated history after a flood like
/// `cargo run`.
const REPLAY_CAP_BYTES: usize = 8 * 1024 * 1024;

/// Terminal-query escape sequences that expect a reply from the emulator
/// (CSI DSR `\x1b[6n` etc, CSI DA `\x1b[c`/`\x1b[>c`, OSC color `\x1b]10;?\x07`).
/// On *re-attach* of a shell PTY we strip these from the snapshot — xterm.js
/// would otherwise re-process the query, emit a reply via `onData`, and the
/// shell sitting at a cooked-mode prompt would echo it back as literal text
/// (`^[[1;1R`) polluting the visible buffer. The original session already
/// received its reply on first attach; replay must be a pure visual restore.
///
/// Never apply to claude PTYs (their TUI re-renders the whole screen on every
/// repaint so the queries cause no echo trail) and never on the *first* attach
/// of any PTY — the initial DSR/DA query needs xterm to actually respond, or
/// the child (e.g. claude waiting on DSR-6 at startup) hangs forever.
fn query_strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1b\[[?>=]?\d*(?:;\d*)*[nc]|\x1b\]\d+;\?(?:\x07|\x1b\\)")
            .expect("query strip regex")
    })
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("pty: {0}")]
    Other(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown terminal id: {0}")]
    UnknownTerminal(String),
}

/// What program a PTY is running. The dock (auxiliary shells) filters out
/// `Claude` PTYs because the pure-mode claude TUI is already its own main
/// pane — surfacing it again as a dock tab is just confusing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    Shell,
    Claude,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalInfo {
    pub id: String,
    pub session_id: AggregateId,
    pub title: String,
    pub cwd: String,
    pub kind: TerminalKind,
}

struct LiveTerminal {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    /// Held so the spawned process is waited on / killable.
    _child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    /// OS pid of the direct child (the shell). Used to kill the **whole**
    /// process tree on close — `Child::kill()` only reaps the shell itself,
    /// leaving grandchildren like a `cargo run` app orphaned and alive.
    pid: Option<u32>,
    session_id: AggregateId,
    title: String,
    cwd: String,
    kind: TerminalKind,
    replay: Arc<Mutex<ReplayBuffer>>,
    /// Pure-mode signal sniffer — present only for `Claude` PTYs. The reader
    /// thread feeds it raw output; it drives the `session:<id>:pure-state` dot
    /// snapshot and pushes marker edges to the auto-pilot sink. `write` resets
    /// its latches when the user submits a turn. `None` for shells. See
    /// `infra::pure_signals`.
    pure: Option<Arc<Mutex<PureSniffer>>>,
    /// Output-silence clock for claude PTYs — stamped on each content-bearing
    /// chunk so `compute_pure_state` can drop a quiet turn out of "busy" even
    /// when a stale spinner frame lingers in the sniffer tail. `None` for shells.
    idle: Option<Arc<Mutex<IdleState>>>,
    /// App handle kept so takeover acquire/release can emit
    /// `session:<id>:takeover` without threading an `AppHandle` through every
    /// supervisor method.
    app: AppHandle,
    /// Last desktop-driven size `(cols, rows)`. Updated by the desktop `resize`
    /// path and seeded at spawn. When a phone takes over and resizes the PTY to
    /// its own dimensions, releasing the takeover restores this so the desktop
    /// view comes back at the size it had.
    desktop_size: Mutex<(u16, u16)>,
}

/// Output-silence clock for a claude PTY. The reader stamps `last_output` on
/// every *content-bearing* chunk (chrome redraws — the animated "Tip:" hint, a
/// cursor blink — don't count, see [`has_content`]). The dot snapshot
/// ([`compute_pure_state`]) treats a turn whose output has gone quiet past
/// `IDLE_DONE_MS` as no longer busy, so a finished turn that left a spinner-shaped
/// frame in the sniffer's append-only tail still drops out of "busy".
struct IdleState {
    last_output: Instant,
    /// Last instant the current TUI frame showed a running turn
    /// ([`PureSniffer::is_busy_now`]). claude interleaves response text and the
    /// spinner/interrupt footer, so `is_busy_now` flickers false on frames whose
    /// last line is content — which made the dot flap blue→settled→blue mid-turn.
    /// Treating "busy seen within [`BUSY_HYSTERESIS_MS`]" as still busy smooths
    /// that at the source. Seeded in the past so a fresh PTY isn't born "busy".
    last_busy: Instant,
}

/// Output silence (ms) after which a turn with no live spinner/interrupt footer
/// is treated as done. A live spinner repaints as content, so a genuinely
/// thinking turn keeps refreshing the clock and stays "busy".
const IDLE_DONE_MS: u64 = 2500;

/// How long after the last "turn is running" frame the dot stays busy even if the
/// current frame's last line is content rather than the spinner/interrupt footer.
/// Absorbs the per-frame `is_busy_now` flicker without noticeably delaying the
/// real settle (the silence clock, [`IDLE_DONE_MS`], still bounds it).
const BUSY_HYSTERESIS_MS: u64 = 1200;

/// Heartbeat (ms) at which the pure-state snapshot is re-evaluated and re-emitted
/// if it changed. Self-heals a snapshot the frontend missed (session switch,
/// panel remount) and is what makes a silence-based turn-end land without a fresh
/// PTY chunk to drive the reader. Comfortably under the frontend recency window.
const HEARTBEAT_MS: u64 = 750;

/// Bytes that the reader thread has emitted so far, kept around so a late
/// frontend listener can catch up on early output (shell banner / first
/// prompt) without losing anything. Each emit is tagged with `last_seq` so
/// the frontend can deduplicate against live events that arrive while the
/// snapshot is in flight.
#[derive(Default)]
struct ReplayBuffer {
    data: VecDeque<u8>,
    last_seq: u64,
    /// How many times the frontend has called `attach_snapshot` on this PTY.
    /// `0` means "first attach" — the query bytes must ride through verbatim so
    /// xterm.js replies and unblocks the child. Any subsequent call is treated
    /// as a re-attach; see `query_strip_re` for the kind-gated strip.
    attach_count: u32,
}

#[derive(Debug, Clone, Serialize)]
struct OutputEvent {
    seq: u64,
    data: String,
}

/// A single PTY output chunk, fanned out to mobile-takeover subscribers over a
/// [`broadcast`] channel alongside the desktop's `terminal:<id>:output` Tauri
/// event. Carries the terminal id so one shared channel serves every terminal —
/// the mobile WS handler filters to the terminal it mirrors. `seq` mirrors the
/// replay buffer's sequence so a mobile client can dedup against the snapshot it
/// fetched on attach, exactly like the desktop frontend does.
#[derive(Debug, Clone)]
pub struct TerminalOutput {
    pub terminal_id: String,
    pub seq: u64,
    pub data: String,
}

/// Payload of the `session:<id>:takeover` Tauri event. The desktop frontend
/// listens for this to freeze/unfreeze its terminal view when a phone takes over
/// (or releases) a pure session's PTY. `by` names the actor for future UIs
/// (always `"mobile"` today).
#[derive(Debug, Clone, Serialize)]
struct TakeoverEvent {
    active: bool,
    by: &'static str,
}

#[derive(Debug, Serialize)]
pub struct TerminalAttachSnapshot {
    pub data: String,
    pub last_seq: u64,
}

/// Current pure-turn dot state for a session, reconstructed from the live
/// `PureSniffer` + output clock. Doubles as the payload of the
/// `session:<id>:pure-state` Tauri event: the backend emits this LEVEL snapshot
/// (not an edge) whenever it changes, plus on a heartbeat, so a snapshot the
/// frontend missed self-heals on the next tick. The frontend also calls
/// `claude_pure_state` on attach to seed the dot before the first event.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PureState {
    /// A prompt/menu is on screen — the red "wants input" dot.
    pub needs_input: bool,
    /// A turn is in flight with no prompt waiting — the blue "busy" dot.
    pub busy: bool,
}

/// Compute the pure-turn dot snapshot from the live sniffer + output clock.
/// `needs_input` (red) outranks `busy` (blue). A turn is "busy" only while the
/// current TUI frame shows it running ([`PureSniffer::is_busy_now`]) AND output
/// hasn't gone silent past `IDLE_DONE_MS` — the silence guard unsticks a finished
/// turn whose last painted frame still looks like a spinner.
fn compute_pure_state(sniffer: &PureSniffer, idle: &IdleState) -> PureState {
    let needs_input = sniffer.prompt_open();
    let fresh = idle.last_output.elapsed() < Duration::from_millis(IDLE_DONE_MS);
    // Hysteresis: a turn counts as running if the current frame shows it OR one
    // did within BUSY_HYSTERESIS_MS — so content-last-line frames mid-turn don't
    // momentarily read "settled" and flap the dot.
    let busy_recent = idle.last_busy.elapsed() < Duration::from_millis(BUSY_HYSTERESIS_MS);
    let busy = !needs_input && fresh && (sniffer.is_busy_now() || busy_recent);
    PureState { needs_input, busy }
}

/// What to launch inside a freshly-opened PTY.
enum PtyProgram {
    /// The user's login shell (pwsh/cmd on Windows, login shell in WSL).
    Shell,
    /// The interactive `claude` TUI with our index/workspace flags — the
    /// "Claude Code puro" mode.
    Claude(ClaudePtyOpts),
}

impl PtyProgram {
    fn title_prefix(&self) -> &'static str {
        match self {
            PtyProgram::Shell => "Terminal",
            PtyProgram::Claude(_) => "Claude",
        }
    }

    fn kind(&self) -> TerminalKind {
        match self {
            PtyProgram::Shell => TerminalKind::Shell,
            PtyProgram::Claude(_) => TerminalKind::Claude,
        }
    }
}

/// Flags handed to the interactive `claude` process in pure mode. Mirrors the
/// subset of the stream-json adapter's options that make sense for a TUI.
#[derive(Debug, Clone, Default)]
pub struct ClaudePtyOpts {
    /// `--session-id <uuid>`. Pinning it to our session aggregate id makes
    /// claude write its transcript at a path we can find (`<id>.jsonl`), which
    /// is how pure-mode sessions get an auto-title. Empty → claude picks one.
    pub session_id: String,
    /// When the transcript for `session_id` already exists on disk (a resumed
    /// session — e.g. after an app restart), claude rejects `--session-id`
    /// with "Session ID … is already in use". Resume it with `--resume <id>`
    /// instead, which keeps writing to the same `<id>.jsonl`.
    pub resume: bool,
    /// Empty → let claude pick its default model.
    pub model: String,
    /// e.g. "default" | "acceptEdits" | "bypassPermissions" | "plan". Empty →
    /// claude's interactive default. "bypassPermissions" is emitted as the bare
    /// `--dangerously-skip-permissions` flag (see `build_pty_args`).
    pub permission_mode: String,
    /// `--mcp-config <path>` when present (oxyris index/LSP server).
    pub mcp_config_path: Option<String>,
    /// `--append-system-prompt <text>` when present (MCP tool nudge).
    pub system_prompt: Option<String>,
}

/// Build the `CommandBuilder` for a PTY program, resolving the right binary
/// for the environment. Mirrors the binary-resolution the stream-json adapter
/// does for `claude` (npm shim is usually `claude.cmd`, which CreateProcess
/// can't launch directly — forward through `cmd.exe /C`).
fn build_cmd(
    env: &Environment,
    cwd: &str,
    extra_env: &[(String, String)],
    program: &PtyProgram,
) -> Result<CommandBuilder, PtyError> {
    match (env, program) {
        (Environment::Local, PtyProgram::Shell) => {
            #[cfg(windows)]
            let mut cmd = {
                // PowerShell first, fall back to cmd.
                let pwsh = which::which("pwsh.exe").or_else(|_| which::which("powershell.exe"));
                match pwsh {
                    Ok(p) => {
                        let mut c = CommandBuilder::new(p.to_string_lossy().into_owned());
                        // Mute the PSReadLine "ding". Its BellStyle defaults to
                        // Audible, which calls [Console]::Beep host-side (during
                        // startup VT probing and on empty-buffer/failed edits) —
                        // that's the beep heard every time a shell PTY spawns or a
                        // thread/terminal regains focus and respawns. The bell rings
                        // out-of-band via Win32 Beep(), so it can't be filtered from
                        // the xterm byte stream; the only fix is at the source.
                        // -NoExit keeps the REPL interactive; the user profile still
                        // loads first, so this overrides whatever it sets. try/catch
                        // swallows the error if PSReadLine isn't present.
                        c.arg("-NoExit");
                        c.arg("-Command");
                        c.arg("try { Set-PSReadLineOption -BellStyle None } catch {}");
                        c
                    }
                    Err(_) => CommandBuilder::new("cmd.exe"),
                }
            };
            #[cfg(not(windows))]
            let mut cmd = {
                // The user's login shell ($SHELL), falling back to bash. `-l`
                // loads the login profile so PATH/aliases match a real terminal.
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
                let mut c = CommandBuilder::new(shell);
                c.arg("-l");
                c
            };
            cmd.cwd(cwd);
            apply_env(&mut cmd, extra_env);
            Ok(cmd)
        }
        (Environment::Wsl { distro }, PtyProgram::Shell) => {
            // Launch wsl.exe with the distro + cwd; the Linux side picks the
            // user's login shell automatically. ConPTY hosts the pty and
            // bridges bytes to wsl.exe — no agent needed for interactive use.
            let wsl = wsl_exe();
            let mut cmd = CommandBuilder::new(wsl);
            cmd.args(["-d", distro, "--cd", cwd]);
            apply_wslenv(&mut cmd, extra_env);
            Ok(cmd)
        }
        (Environment::Local, PtyProgram::Claude(opts)) => {
            let full = which::which("claude")
                .or_else(|_| which::which("claude.cmd"))
                .or_else(|_| which::which("claude.exe"))
                .map_err(|e| {
                    PtyError::Other(format!(
                        "claude not found on PATH (checked claude, claude.cmd, claude.exe): {e}"
                    ))
                })?;
            let is_batch = full
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| matches!(x.to_ascii_lowercase().as_str(), "cmd" | "bat"))
                .unwrap_or(false);
            let mut cmd = if is_batch {
                let mut c = CommandBuilder::new("cmd.exe");
                c.arg("/C");
                c.arg(full.as_os_str());
                c
            } else {
                CommandBuilder::new(full.as_os_str())
            };
            for a in claude_args(opts) {
                cmd.arg(a);
            }
            cmd.cwd(cwd);
            apply_env(&mut cmd, extra_env);
            Ok(cmd)
        }
        (Environment::Wsl { distro }, PtyProgram::Claude(opts)) => {
            // Run through a login shell inside the distro, mirroring the
            // stream-json adapter. We can't pass `wsl -- claude <args>`
            // directly because the system-prompt nudge is a multiline string
            // with backticks — handed to wsl bare it gets interpreted by the
            // shell. `sh -lc` with single-quote escaping neutralises that, and
            // `-l` puts claude on PATH.
            //
            // The cwd is already a POSIX path for WSL projects (worktree paths
            // are created inside the distro), so it is used verbatim — running
            // it through `wslpath -u` would double-translate `/home/...` into
            // `/mnt/c/home/...` and break `cd`.
            let args = claude_args(opts)
                .iter()
                .map(|a| shell_escape(a))
                .collect::<Vec<_>>()
                .join(" ");
            let script = format!("cd {} && exec claude {}", shell_escape(cwd), args);
            let mut cmd = CommandBuilder::new(wsl_exe());
            cmd.args(["-d", distro, "--", "sh", "-lc", &script]);
            apply_wslenv(&mut cmd, extra_env);
            Ok(cmd)
        }
    }
}

/// Single-quote a value for a POSIX shell so backticks, spaces, newlines and
/// `$()` inside it stay literal. Bare-word fast path for simple tokens.
fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn wsl_exe() -> String {
    which::which("wsl.exe")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "wsl.exe".into())
}

fn claude_args(opts: &ClaudePtyOpts) -> Vec<String> {
    let mut args = Vec::new();
    if !opts.session_id.trim().is_empty() {
        // Existing transcript → resume it; fresh → pin the id so the transcript
        // lands at the path we auto-title from. See `ClaudePtyOpts::resume`.
        args.push(if opts.resume {
            "--resume".into()
        } else {
            "--session-id".into()
        });
        args.push(opts.session_id.clone());
    }
    if !opts.model.trim().is_empty() {
        args.push("--model".into());
        args.push(opts.model.clone());
    }
    if !opts.permission_mode.trim().is_empty() {
        // FullAccess maps to the real "dangerous" bare flag rather than
        // `--permission-mode bypassPermissions`, which the CLI refuses in some
        // contexts. Every other mode passes through as a `--permission-mode`.
        if opts.permission_mode == "bypassPermissions" {
            args.push("--dangerously-skip-permissions".into());
        } else {
            args.push("--permission-mode".into());
            args.push(opts.permission_mode.clone());
        }
    }
    if let Some(p) = &opts.mcp_config_path {
        args.push("--mcp-config".into());
        args.push(p.clone());
    }
    if let Some(s) = &opts.system_prompt {
        args.push("--append-system-prompt".into());
        args.push(s.clone());
    }
    args
}

fn apply_env(cmd: &mut CommandBuilder, extra_env: &[(String, String)]) {
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
}

/// wsl.exe forwards `WSLENV=NAME/u:OTHER/u` over the boundary; listing our
/// vars there makes them appear inside the distro.
fn apply_wslenv(cmd: &mut CommandBuilder, extra_env: &[(String, String)]) {
    if extra_env.is_empty() {
        return;
    }
    let names: Vec<String> = extra_env.iter().map(|(k, _)| format!("{k}/u")).collect();
    cmd.env("WSLENV", names.join(":"));
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
}

pub struct PtySupervisor {
    terminals: Mutex<HashMap<String, LiveTerminal>>,
    /// Optional in-process sink for pure-signal notices (the auto-pilot
    /// controller). Set once at boot via [`PtySupervisor::set_signal_sink`].
    signal_sink: Mutex<Option<UnboundedSender<PureSignalNotice>>>,
    /// Fan-out of every PTY's output to mobile-takeover subscribers. One shared
    /// channel for all terminals; receivers filter by `terminal_id`. The reader
    /// thread only publishes when there is at least one receiver, so the desktop
    /// pays nothing when no phone is connected.
    output_tx: broadcast::Sender<TerminalOutput>,
    /// Terminal ids currently under mobile takeover. While present, the desktop
    /// `write`/`resize` paths no-op (the phone owns the PTY) and the desktop UI
    /// shows a frozen overlay.
    takeover: Mutex<HashSet<String>>,
}

impl Default for PtySupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl PtySupervisor {
    pub fn new() -> Self {
        let (output_tx, _) = broadcast::channel(OUTPUT_BROADCAST_CAP);
        Self {
            terminals: Mutex::new(HashMap::new()),
            signal_sink: Mutex::new(None),
            output_tx,
            takeover: Mutex::new(HashSet::new()),
        }
    }

    /// Subscribe to the output of every live terminal. The mobile-takeover WS
    /// handler filters the stream to the one terminal it mirrors. Pair with
    /// [`PtySupervisor::attach_snapshot`] for the initial backfill, then drop any
    /// streamed chunk whose `seq` is `<=` the snapshot's `last_seq`.
    pub fn subscribe_output(&self) -> broadcast::Receiver<TerminalOutput> {
        self.output_tx.subscribe()
    }

    /// Wire the auto-pilot controller's notice channel. Called once at boot,
    /// before any PTY spawns, so every reader thread captures it.
    pub fn set_signal_sink(&self, tx: UnboundedSender<PureSignalNotice>) {
        if let Ok(mut slot) = self.signal_sink.lock() {
            *slot = Some(tx);
        }
    }

    /// ANSI-stripped tail of a terminal's scrollback, capped at `max_chars`.
    /// Used to build the auto-pilot's transcript context. Does not touch
    /// `attach_count` (unlike `attach_snapshot`), so it's safe to poll.
    pub fn scrollback_tail(&self, id: &str, max_chars: usize) -> Option<String> {
        let terminals = self.terminals.lock().ok()?;
        let live = terminals.get(id)?;
        let rb = live.replay.lock().ok()?;
        let raw =
            String::from_utf8_lossy(&rb.data.iter().copied().collect::<Vec<u8>>()).into_owned();
        let stripped = strip_ansi(&raw);
        let len = stripped.chars().count();
        if len <= max_chars {
            return Some(stripped);
        }
        let start = stripped
            .char_indices()
            .nth(len - max_chars)
            .map(|(i, _)| i)
            .unwrap_or(0);
        Some(stripped[start..].to_owned())
    }

    /// Spawn a new terminal in `cwd`. Returns its id; output streams as
    /// Tauri events on `terminal:<id>:output` (binary chunks as base64?
    /// for now we send UTF-8 lossy strings — fine for ASCII / VT-aware
    /// xterm.js). When the child exits, we emit `terminal:<id>:exit`.
    ///
    /// `extra_env` lets callers inject `OXYRIS_*` (Docker per-worktree env)
    /// or other variables into the spawned shell.
    pub fn spawn(
        &self,
        app: AppHandle,
        env: &Environment,
        session_id: AggregateId,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalInfo, PtyError> {
        self.spawn_with_env(app, env, session_id, cwd, cols, rows, &[])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_env(
        &self,
        app: AppHandle,
        env: &Environment,
        session_id: AggregateId,
        cwd: &str,
        cols: u16,
        rows: u16,
        extra_env: &[(String, String)],
    ) -> Result<TerminalInfo, PtyError> {
        self.spawn_program(
            app,
            env,
            session_id,
            cwd,
            cols,
            rows,
            extra_env,
            PtyProgram::Shell,
        )
    }

    /// Spawn the provider's interactive TUI (`claude`) directly in a PTY —
    /// the "Claude Code puro" mode. Same plumbing as a shell terminal; only
    /// the launched program differs. MCP/system-prompt/model flags are passed
    /// through `opts` so the pure session still gets our index + workspace.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_claude(
        &self,
        app: AppHandle,
        env: &Environment,
        session_id: AggregateId,
        cwd: &str,
        cols: u16,
        rows: u16,
        extra_env: &[(String, String)],
        opts: ClaudePtyOpts,
    ) -> Result<TerminalInfo, PtyError> {
        self.spawn_program(
            app,
            env,
            session_id,
            cwd,
            cols,
            rows,
            extra_env,
            PtyProgram::Claude(opts),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_program(
        &self,
        app: AppHandle,
        env: &Environment,
        session_id: AggregateId,
        cwd: &str,
        cols: u16,
        rows: u16,
        extra_env: &[(String, String)],
        program: PtyProgram,
    ) -> Result<TerminalInfo, PtyError> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Other(e.to_string()))?;

        let title_prefix = program.title_prefix();
        let kind = program.kind();
        let cmd = build_cmd(env, cwd, extra_env, &program)?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Other(e.to_string()))?;
        // Grab the pid before wrapping the child in a mutex — needed to kill
        // the whole descendant tree on close (not just this direct child).
        let pid = child.process_id();
        // The slave handle is no longer needed once spawn_command consumes it.
        drop(pair.slave);

        let id = format!("term-{}", Uuid::now_v7());
        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|e| PtyError::Other(e.to_string()))?,
        ));
        let master = Arc::new(Mutex::new(pair.master));
        let child = Arc::new(Mutex::new(child));
        let replay = Arc::new(Mutex::new(ReplayBuffer::default()));
        // Only claude PTYs get a pure-signal sniffer + idle watchdog — shells
        // have no TUI prompts/turns to detect.
        let is_claude = matches!(kind, TerminalKind::Claude);
        let pure: Option<Arc<Mutex<PureSniffer>>> =
            is_claude.then(|| Arc::new(Mutex::new(PureSniffer::new())));
        let idle: Option<Arc<Mutex<IdleState>>> = is_claude.then(|| {
            Arc::new(Mutex::new(IdleState {
                last_output: Instant::now(),
                // Seed in the past so a freshly spawned, idle claude isn't born
                // "busy" by the hysteresis window.
                last_busy: Instant::now()
                    .checked_sub(Duration::from_millis(BUSY_HYSTERESIS_MS))
                    .unwrap_or_else(Instant::now),
            }))
        });
        // Last pure-state snapshot emitted on `session:<id>:pure-state`, shared
        // between the reader (immediate, on output) and the heartbeat (self-heal
        // / silence-based turn-end). `None` until the first emit.
        let last_state: Option<Arc<Mutex<Option<PureState>>>> =
            is_claude.then(|| Arc::new(Mutex::new(None)));
        // Lets the reader/heartbeat threads stop themselves when the PTY hits EOF.
        let alive = Arc::new(AtomicBool::new(true));

        // Reader task — pulls bytes off the PTY, appends them to the replay
        // buffer (so a late attach can catch up), and forwards as events.
        let reader_app = app.clone();
        let reader_id = id.clone();
        let reader_master = master.clone();
        let reader_replay = replay.clone();
        let reader_pure = pure.clone();
        let reader_idle = idle.clone();
        let reader_last_state = last_state.clone();
        let reader_alive = alive.clone();
        let reader_session = session_id;
        // Snapshot the auto-pilot sink (if any) so signal notices reach the
        // controller in-process, not just the frontend via Tauri events.
        let reader_sink = self.signal_sink.lock().ok().and_then(|g| g.clone());
        // Mobile-takeover fan-out. Clone of the shared sender; the reader only
        // publishes when a phone is actually subscribed (`receiver_count > 0`),
        // so the desktop-only path stays clone-free.
        let reader_output_tx = self.output_tx.clone();
        std::thread::spawn(move || {
            let mut reader = {
                let Ok(guard) = reader_master.lock() else {
                    return;
                };
                match guard.try_clone_reader() {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = reader_app.emit(
                            &format!("terminal:{reader_id}:exit"),
                            format!("clone reader failed: {e}"),
                        );
                        return;
                    }
                }
            };
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let bytes = &buf[..n];
                        let chunk = String::from_utf8_lossy(bytes).into_owned();
                        let seq = {
                            let mut rb = match reader_replay.lock() {
                                Ok(g) => g,
                                Err(_) => break,
                            };
                            rb.last_seq += 1;
                            rb.data.extend(bytes.iter().copied());
                            // Drain the overflow in one shot. The old one-byte
                            // `pop_front` loop was O(n) per chunk and could sever
                            // a multi-byte UTF-8 char or an ANSI escape sequence
                            // at the front, so the replayed snapshot started
                            // mid-escape and rendered garbled.
                            if rb.data.len() > REPLAY_CAP_BYTES {
                                let excess = rb.data.len() - REPLAY_CAP_BYTES;
                                rb.data.drain(..excess);
                            }
                            rb.last_seq
                        };
                        // Reduce the chunk for the detectors ONCE (claude PTYs
                        // only), before we hand the owned chunk to the output
                        // emit — so the strip runs a single time and the emit
                        // doesn't need a clone.
                        let stripped = reader_pure.as_ref().map(|_| strip_for_detection(&chunk));
                        // Mobile-takeover fan-out — only pay the clone when a
                        // phone is actually subscribed. Same (seq, data) the
                        // desktop event carries, tagged with the terminal id.
                        if reader_output_tx.receiver_count() > 0 {
                            let _ = reader_output_tx.send(TerminalOutput {
                                terminal_id: reader_id.clone(),
                                seq,
                                data: chunk.clone(),
                            });
                        }
                        let _ = reader_app.emit(
                            &format!("terminal:{reader_id}:output"),
                            OutputEvent { seq, data: chunk },
                        );
                        // Pure-mode detection: feed the stripped chunk, push
                        // marker edges to the auto-pilot controller (in-process),
                        // and emit the LEVEL dot snapshot when it changes. The
                        // frontend dot rides the snapshot, not the edges — a
                        // missed snapshot self-heals on the heartbeat below.
                        // Computed under the sniffer/idle/last locks, emitted
                        // after they all drop — the Tauri emit must never run
                        // while holding those mutexes.
                        let mut to_emit: Option<PureState> = None;
                        if let (Some(pure), Some(stripped)) = (&reader_pure, &stripped)
                            && let Ok(mut sniffer) = pure.lock()
                        {
                            let signals = sniffer.feed_stripped(stripped);
                            // Auto-pilot still reacts to marker edges (a prompt
                            // to answer / a settled turn). Independent of the dot.
                            if let Some(sink) = &reader_sink {
                                for signal in &signals {
                                    let _ = sink.send(PureSignalNotice {
                                        session_id: reader_session,
                                        terminal_id: reader_id.clone(),
                                        signal: *signal,
                                    });
                                }
                            }
                            // Output clock + dot snapshot. A content-bearing chunk
                            // pushes the silence window out; a chrome-only redraw
                            // (animated "Tip:" hint, cursor blink) must NOT, or a
                            // finished-but-animating turn never goes quiet and
                            // sticks "busy" (blue dot).
                            if let (Some(idle), Some(last)) = (&reader_idle, &reader_last_state)
                                && let (Ok(mut st), Ok(mut prev)) = (idle.lock(), last.lock())
                            {
                                if has_content_stripped(stripped) {
                                    st.last_output = Instant::now();
                                }
                                // Stamp the running-turn clock whenever this frame
                                // shows a live turn, so the hysteresis in
                                // `compute_pure_state` can hold the dot busy across
                                // a content-last-line frame without flapping.
                                if sniffer.is_busy_now() {
                                    st.last_busy = Instant::now();
                                }
                                let next = compute_pure_state(&sniffer, &st);
                                if *prev != Some(next) {
                                    *prev = Some(next);
                                    to_emit = Some(next);
                                }
                            }
                        }
                        if let Some(next) = to_emit {
                            let _ = reader_app
                                .emit(&format!("session:{reader_session}:pure-state"), next);
                        }
                    }
                    Err(_) => break,
                }
            }
            reader_alive.store(false, Ordering::Relaxed);
            let _ = reader_app.emit(&format!("terminal:{reader_id}:exit"), "eof");
        });

        // Heartbeat (claude PTYs only) — re-evaluates the pure-state snapshot on a
        // fixed tick and re-emits it when it changed. Two jobs: (1) self-heal a
        // snapshot the frontend missed because no listener was attached when the
        // reader emitted it (session switch, panel remount); (2) land a
        // silence-based turn-end — when a turn finishes by going quiet past
        // `IDLE_DONE_MS` with no fresh PTY chunk to drive the reader, the clock in
        // `compute_pure_state` flips `busy` false and this tick ships it. Runs off
        // the same byte pipe as the reader, so it survives the window losing focus.
        if let (Some(idle), Some(pure), Some(last)) =
            (idle.clone(), pure.clone(), last_state.clone())
        {
            let hb_app = app.clone();
            let hb_alive = alive.clone();
            let hb_session = session_id;
            std::thread::spawn(move || {
                while hb_alive.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(HEARTBEAT_MS));
                    let emit = {
                        let (Ok(sniffer), Ok(st), Ok(mut prev)) =
                            (pure.lock(), idle.lock(), last.lock())
                        else {
                            break;
                        };
                        let next = compute_pure_state(&sniffer, &st);
                        if *prev != Some(next) {
                            *prev = Some(next);
                            Some(next)
                        } else {
                            None
                        }
                    };
                    if let Some(next) = emit {
                        let _ = hb_app.emit(&format!("session:{hb_session}:pure-state"), next);
                    }
                }
            });
        }

        // Reaper — waits on the child so the process doesn't zombie.
        let reaper_child = child.clone();
        std::thread::spawn(move || {
            if let Ok(mut child) = reaper_child.lock() {
                let _ = child.wait();
            }
        });

        // Pick a per-session, per-kind sequential title so shells number from
        // 1 (Terminal 1, Terminal 2, …) independently of the claude PTY.
        let mut terminals = self.terminals.lock().expect("pty mutex poisoned");
        let next_index = terminals
            .values()
            .filter(|t| t.session_id == session_id && t.kind == kind)
            .count()
            + 1;
        let title = format!("{title_prefix} {next_index}");
        let cwd_owned = cwd.to_owned();
        terminals.insert(
            id.clone(),
            LiveTerminal {
                writer,
                master,
                _child: child,
                pid,
                session_id,
                title: title.clone(),
                cwd: cwd_owned.clone(),
                kind,
                replay,
                pure,
                idle,
                app,
                desktop_size: Mutex::new((cols, rows)),
            },
        );

        Ok(TerminalInfo {
            id,
            session_id,
            title,
            cwd: cwd_owned,
            kind,
        })
    }

    /// Snapshot of everything the reader has emitted so far on `id`, plus
    /// the highest sequence number it carries. The frontend writes the data
    /// to its xterm and then ignores any live `output` events whose `seq` is
    /// not greater than `last_seq`, eliminating the attach-vs-emit race.
    pub fn attach_snapshot(&self, id: &str) -> Result<TerminalAttachSnapshot, PtyError> {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        let live = terminals
            .get(id)
            .ok_or_else(|| PtyError::UnknownTerminal(id.to_owned()))?;
        let kind = live.kind;
        let mut rb = live.replay.lock().expect("pty replay mutex poisoned");
        let raw =
            String::from_utf8_lossy(&rb.data.iter().copied().collect::<Vec<u8>>()).into_owned();
        // Strip terminal queries only on shell re-attach. See `query_strip_re`
        // for the reasoning (first attach must be raw, claude TUIs always raw).
        let data = if matches!(kind, TerminalKind::Shell) && rb.attach_count > 0 {
            query_strip_re().replace_all(&raw, "").into_owned()
        } else {
            raw
        };
        rb.attach_count = rb.attach_count.saturating_add(1);
        Ok(TerminalAttachSnapshot {
            data,
            last_seq: rb.last_seq,
        })
    }

    /// Ground-truth pure-turn dot state for `session_id`, read straight off the
    /// live sniffer + output clock. Lets the frontend seed/reconcile the dot on
    /// attach, before the first `pure-state` event lands. No claude PTY for the
    /// session → the default (idle) state.
    pub fn pure_state(&self, session_id: AggregateId) -> PureState {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        for t in terminals.values() {
            if t.session_id != session_id || t.kind != TerminalKind::Claude {
                continue;
            }
            if let (Some(pure), Some(idle)) = (&t.pure, &t.idle)
                && let (Ok(sniffer), Ok(st)) = (pure.lock(), idle.lock())
            {
                return compute_pure_state(&sniffer, &st);
            }
        }
        PureState::default()
    }

    /// Snapshot of every live terminal that belongs to a given session.
    pub fn list_for_session(&self, session_id: AggregateId) -> Vec<TerminalInfo> {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        terminals
            .iter()
            .filter(|(_, t)| t.session_id == session_id)
            .map(|(id, t)| TerminalInfo {
                id: id.clone(),
                session_id: t.session_id,
                title: t.title.clone(),
                cwd: t.cwd.clone(),
                kind: t.kind,
            })
            .collect()
    }

    /// Rename a terminal — used by the dock when the user double-clicks a tab.
    pub fn rename(&self, id: &str, title: &str) -> Result<(), PtyError> {
        let mut terminals = self.terminals.lock().expect("pty mutex poisoned");
        let live = terminals
            .get_mut(id)
            .ok_or_else(|| PtyError::UnknownTerminal(id.to_owned()))?;
        live.title = title.to_owned();
        Ok(())
    }

    pub fn write(&self, id: &str, data: &str) -> Result<(), PtyError> {
        // Desktop input is frozen while a phone holds the takeover — the UI
        // shows an overlay, but enforce it here too so a stray keystroke that
        // races the overlay can't reach the PTY the phone is driving.
        if self.is_takeover(id) {
            return Ok(());
        }
        self.write_raw(id, data)
    }

    /// Mobile-takeover input path: write bytes regardless of the takeover lock
    /// (the phone IS the lock holder). The WS handler only calls this after a
    /// successful [`acquire_takeover`].
    pub fn write_mobile(&self, id: &str, data: &str) -> Result<(), PtyError> {
        self.write_raw(id, data)
    }

    fn write_raw(&self, id: &str, data: &str) -> Result<(), PtyError> {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        let live = terminals
            .get(id)
            .ok_or_else(|| PtyError::UnknownTerminal(id.to_owned()))?;
        // A carriage return submits the current turn (answers a prompt / sends a
        // message). Clear the pure-signal latches so the next prompt or turn-end
        // fires fresh, and stamp the output clock so the just-started turn reads
        // "fresh" until claude paints its spinner. Matches the frontend's
        // `onPtyInput` reset on `\r`.
        if data.contains('\r') {
            if let Some(pure) = &live.pure
                && let Ok(mut sniffer) = pure.lock()
            {
                sniffer.reset();
            }
            if let Some(idle) = &live.idle
                && let Ok(mut st) = idle.lock()
            {
                st.last_output = Instant::now();
            }
        }
        let mut writer = live.writer.lock().expect("pty writer mutex poisoned");
        writer.write_all(data.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), PtyError> {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        let live = terminals
            .get(id)
            .ok_or_else(|| PtyError::UnknownTerminal(id.to_owned()))?;
        // Remember the desktop's size even while a phone owns the PTY, so
        // releasing the takeover can restore it. While locked, don't actually
        // resize — the phone's dimensions win until it releases.
        if let Ok(mut ds) = live.desktop_size.lock() {
            *ds = (cols, rows);
        }
        let locked = self
            .takeover
            .lock()
            .map(|t| t.contains(id))
            .unwrap_or(false);
        if locked {
            return Ok(());
        }
        resize_master(live, cols, rows)
    }

    /// Mobile-takeover resize: apply the phone's dimensions to the PTY without
    /// touching the stored desktop size, so [`release_takeover`] can put the
    /// desktop's size back.
    pub fn resize_mobile(&self, id: &str, cols: u16, rows: u16) -> Result<(), PtyError> {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        let live = terminals
            .get(id)
            .ok_or_else(|| PtyError::UnknownTerminal(id.to_owned()))?;
        resize_master(live, cols, rows)
    }

    /// Whether `id` is currently under mobile takeover.
    pub fn is_takeover(&self, id: &str) -> bool {
        self.takeover
            .lock()
            .map(|t| t.contains(id))
            .unwrap_or(false)
    }

    /// Resolve the live `claude` (pure-mode) terminal for a session, if one is
    /// running. Mobile mirrors the pure TUI, so it maps a session id → the
    /// single `Claude` PTY under it.
    pub fn claude_terminal_for_session(&self, session_id: AggregateId) -> Option<String> {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        terminals
            .iter()
            .find(|(_, t)| t.session_id == session_id && t.kind == TerminalKind::Claude)
            .map(|(id, _)| id.clone())
    }

    /// Hand control of a pure session's PTY to a phone. Fails if the terminal is
    /// gone or already under takeover. Emits `session:<id>:takeover {active:true}`
    /// so the desktop view freezes. The phone may now resize/write via the
    /// `*_mobile` paths; the desktop's are no-ops until [`release_takeover`].
    pub fn acquire_takeover(&self, id: &str) -> Result<AggregateId, PtyError> {
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        let live = terminals
            .get(id)
            .ok_or_else(|| PtyError::UnknownTerminal(id.to_owned()))?;
        let session_id = live.session_id;
        let app = live.app.clone();
        {
            let mut t = self.takeover.lock().expect("pty takeover mutex poisoned");
            if t.contains(id) {
                return Err(PtyError::Other("terminal already under takeover".into()));
            }
            t.insert(id.to_owned());
        }
        let _ = app.emit(
            &format!("session:{session_id}:takeover"),
            TakeoverEvent {
                active: true,
                by: "mobile",
            },
        );
        Ok(session_id)
    }

    /// Return control to the desktop: clear the lock, restore the desktop's
    /// pre-takeover size, and emit `session:<id>:takeover {active:false}`. Called
    /// on the phone's explicit release, on WS disconnect, and by the desktop's
    /// force-release command. Idempotent — releasing a terminal that isn't locked
    /// is a no-op.
    pub fn release_takeover(&self, id: &str) {
        {
            let mut t = self.takeover.lock().expect("pty takeover mutex poisoned");
            if !t.remove(id) {
                return;
            }
        }
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        let Some(live) = terminals.get(id) else {
            return;
        };
        let (cols, rows) = live.desktop_size.lock().map(|d| *d).unwrap_or((80, 24));
        let _ = resize_master(live, cols, rows);
        let _ = live.app.emit(
            &format!("session:{}:takeover", live.session_id),
            TakeoverEvent {
                active: false,
                by: "mobile",
            },
        );
    }

    /// Kill is dispatched to a worker thread and returns immediately. The
    /// actual `TerminateProcess` + dropping of the `Box<dyn MasterPty>` /
    /// writer can take a noticeable amount of time on Windows ConPTY (and
    /// in some cases blocks indefinitely while the conpty agent shuts down).
    /// We do **not** want any of that on the IPC thread — a slow kill there
    /// freezes the WebView's response loop.
    pub fn kill(&self, id: &str) -> Result<(), PtyError> {
        let mut terminals = self.terminals.lock().expect("pty mutex poisoned");
        let Some(live) = terminals.remove(id) else {
            return Ok(());
        };
        std::thread::spawn(move || {
            // Kill the whole descendant tree first — `Child::kill()` below only
            // terminates the direct child (the shell), so a `cargo run` app it
            // spawned would survive as an orphan. taskkill /T (Windows) /
            // process-group kill (unix) takes the shell and everything under it.
            if let Some(pid) = live.pid {
                oxyris_procutil::kill_tree(pid);
            }
            if let Ok(mut child) = live._child.lock() {
                let _ = child.kill();
            }
            // `live` drops here on this background thread — drops the writer
            // (closes stdin), then drops the master Arc reference, then the
            // child Arc reference. The reader thread will see EOF and exit
            // on its own.
            drop(live);
        });
        Ok(())
    }
}

/// Apply a size to a terminal's ConPTY master. Shared by the desktop resize, the
/// mobile-takeover resize, and the takeover release (which restores the desktop
/// size). Kept free-standing so all three go through one place.
fn resize_master(live: &LiveTerminal, cols: u16, rows: u16) -> Result<(), PtyError> {
    let master = live.master.lock().expect("pty master mutex poisoned");
    master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyError::Other(e.to_string()))
}
