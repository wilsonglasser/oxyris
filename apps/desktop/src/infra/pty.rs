//! PTY supervisor — owns one ConPTY per terminal id, streams output to the
//! UI as Tauri events, and accepts user input + resize commands.
//!
//! Windows-only in this MVP slice (`Environment::Wsl` returns "not yet
//! supported" until the agent gains a `pty.spawn` op).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use oxyris_core::{AggregateId, Environment};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use uuid::Uuid;

/// Cap on the per-terminal replay buffer. Enough to capture a shell banner +
/// a few screens of output while we wait for the frontend to attach.
const REPLAY_CAP_BYTES: usize = 256 * 1024;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("pty: {0}")]
    Other(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown terminal id: {0}")]
    UnknownTerminal(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalInfo {
    pub id: String,
    pub session_id: AggregateId,
    pub title: String,
    pub cwd: String,
}

struct LiveTerminal {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    /// Held so the spawned process is waited on / killable.
    _child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    session_id: AggregateId,
    title: String,
    cwd: String,
    replay: Arc<Mutex<ReplayBuffer>>,
}

/// Bytes that the reader thread has emitted so far, kept around so a late
/// frontend listener can catch up on early output (shell banner / first
/// prompt) without losing anything. Each emit is tagged with `last_seq` so
/// the frontend can deduplicate against live events that arrive while the
/// snapshot is in flight.
#[derive(Default)]
struct ReplayBuffer {
    data: VecDeque<u8>,
    last_seq: u64,
}

#[derive(Debug, Clone, Serialize)]
struct OutputEvent {
    seq: u64,
    data: String,
}

#[derive(Debug, Serialize)]
pub struct TerminalAttachSnapshot {
    pub data: String,
    pub last_seq: u64,
}

#[derive(Default)]
pub struct PtySupervisor {
    terminals: Mutex<HashMap<String, LiveTerminal>>,
}

impl PtySupervisor {
    pub fn new() -> Self {
        Self::default()
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
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Other(e.to_string()))?;

        let cmd = match env {
            Environment::Windows => {
                // PowerShell first, fall back to cmd.
                let shell = which::which("pwsh.exe")
                    .or_else(|_| which::which("powershell.exe"))
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "cmd.exe".into());
                let mut cmd = CommandBuilder::new(shell);
                cmd.cwd(cwd);
                for (k, v) in extra_env {
                    cmd.env(k, v);
                }
                cmd
            }
            Environment::Wsl { distro } => {
                // Launch wsl.exe with the distro + cwd; the Linux side picks
                // the user's login shell automatically. ConPTY hosts the
                // pty and forwards bytes to wsl.exe's stdin/stdout which
                // bridges to the distro's shell — no agent needed for
                // interactive terminal.
                let wsl = which::which("wsl.exe")
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "wsl.exe".into());
                let mut cmd = CommandBuilder::new(wsl);
                cmd.args(["-d", distro, "--cd", cwd]);
                // wsl.exe forwards `WSLENV=NAME/u:OTHER/u` over the boundary;
                // listing our vars there makes them appear inside the distro.
                if !extra_env.is_empty() {
                    let names: Vec<String> =
                        extra_env.iter().map(|(k, _)| format!("{k}/u")).collect();
                    cmd.env("WSLENV", names.join(":"));
                    for (k, v) in extra_env {
                        cmd.env(k, v);
                    }
                }
                cmd
            }
        };
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Other(e.to_string()))?;
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

        // Reader task — pulls bytes off the PTY, appends them to the replay
        // buffer (so a late attach can catch up), and forwards as events.
        let reader_app = app.clone();
        let reader_id = id.clone();
        let reader_master = master.clone();
        let reader_replay = replay.clone();
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
                            while rb.data.len() > REPLAY_CAP_BYTES {
                                rb.data.pop_front();
                            }
                            rb.last_seq
                        };
                        let _ = reader_app.emit(
                            &format!("terminal:{reader_id}:output"),
                            OutputEvent { seq, data: chunk },
                        );
                    }
                    Err(_) => break,
                }
            }
            let _ = reader_app.emit(&format!("terminal:{reader_id}:exit"), "eof");
        });

        // Reaper — waits on the child so the process doesn't zombie.
        let reaper_child = child.clone();
        std::thread::spawn(move || {
            if let Ok(mut child) = reaper_child.lock() {
                let _ = child.wait();
            }
        });

        // Pick a per-session sequential title (Terminal 1, Terminal 2, ...).
        let mut terminals = self.terminals.lock().expect("pty mutex poisoned");
        let next_index = terminals
            .values()
            .filter(|t| t.session_id == session_id)
            .count()
            + 1;
        let title = format!("Terminal {next_index}");
        let cwd_owned = cwd.to_owned();
        terminals.insert(
            id.clone(),
            LiveTerminal {
                writer,
                master,
                _child: child,
                session_id,
                title: title.clone(),
                cwd: cwd_owned.clone(),
                replay,
            },
        );

        Ok(TerminalInfo {
            id,
            session_id,
            title,
            cwd: cwd_owned,
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
        let rb = live.replay.lock().expect("pty replay mutex poisoned");
        let data =
            String::from_utf8_lossy(&rb.data.iter().copied().collect::<Vec<u8>>()).into_owned();
        Ok(TerminalAttachSnapshot {
            data,
            last_seq: rb.last_seq,
        })
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
        let terminals = self.terminals.lock().expect("pty mutex poisoned");
        let live = terminals
            .get(id)
            .ok_or_else(|| PtyError::UnknownTerminal(id.to_owned()))?;
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
        let master = live.master.lock().expect("pty master mutex poisoned");
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Other(e.to_string()))?;
        Ok(())
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
