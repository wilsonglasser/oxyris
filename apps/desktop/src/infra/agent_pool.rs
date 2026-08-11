//! Per-distro WSL agent supervisor.
//!
//! One `oxyris-agent` process lives inside each WSL distro that hosts an
//! Oxyris project. The pool spawns the agent on demand, routes NDJSON
//! requests/results over its stdio, and re-deploys the binary when it's
//! missing from the distro.
//!
//! **Deploy strategy.** The backend ships with a Linux x64 musl agent binary
//! at `OXYRIS_AGENT_BIN_PATH` (overridable for dev). On first use in a
//! distro, we:
//!
//! 1. Copy the binary into `~/.oxyris/bin/oxyris-agent` inside the distro,
//! 2. `chmod +x` it,
//! 3. Spawn it via `wsl.exe -d <distro> -- ~/.oxyris/bin/oxyris-agent`.
//!
//! The copy goes through `wsl.exe` using `/mnt/c/...` translation instead of
//! `\\wsl.localhost` to keep the 9p path out of the loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use oxyris_ipc::{Frame, RequestFrame};
use oxyris_procutil::HideConsole;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent binary not found at {path}")]
    BinaryMissing { path: String },
    #[error("spawn wsl.exe: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("deploy step failed: {stage}: {stderr}")]
    DeployFailed { stage: &'static str, stderr: String },
    #[error("agent returned error: {code}: {message}")]
    Remote { code: String, message: String },
    #[error("agent stdio closed unexpectedly")]
    AgentGone,
    #[error("rpc channel closed")]
    Channel,
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
}

struct PendingCall {
    events: Option<mpsc::UnboundedSender<Value>>,
    result: oneshot::Sender<Result<Value, AgentError>>,
}

struct AgentHandle {
    stdin_tx: mpsc::Sender<String>,
    pending: Arc<Mutex<HashMap<String, PendingCall>>>,
    // The child and the reader task live here so they don't drop while the
    // handle is in the map.
    _child: Child,
    _task: tokio::task::JoinHandle<()>,
}

pub struct AgentPool {
    host_agent_path: PathBuf,
    agents: Mutex<HashMap<String, Arc<AgentHandle>>>,
}

impl AgentPool {
    pub fn new(host_agent_path: PathBuf) -> Self {
        Self {
            host_agent_path,
            agents: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve the host agent path. Order:
    ///
    /// 1. `OXYRIS_AGENT_BIN_PATH` env var (explicit override, wins always).
    /// 2. `./dist/agent/oxyris-agent` found by walking up from the current
    ///    executable — covers the dev flow where `scripts/build-agent-linux.ps1`
    ///    deposits the binary at the workspace root.
    /// 3. The caller-supplied `default` fallback (production install path).
    ///
    /// This is what `AppState::initialize` should call.
    pub fn resolve_host_agent_path(default: PathBuf) -> PathBuf {
        if let Some(env) = std::env::var_os("OXYRIS_AGENT_BIN_PATH") {
            return PathBuf::from(env);
        }
        if let Some(dev) = find_dev_agent_binary() {
            return dev;
        }
        default
    }

    pub async fn call(&self, distro: &str, op: &str, args: Value) -> Result<Value, AgentError> {
        let agent = self.ensure_agent(distro).await?;
        let (result_tx, result_rx) = oneshot::channel();
        let id = new_request_id();

        {
            let mut pending = agent.pending.lock().await;
            pending.insert(
                id.clone(),
                PendingCall {
                    events: None,
                    result: result_tx,
                },
            );
        }

        let frame = Frame::Request(RequestFrame {
            id: id.clone(),
            op: op.to_owned(),
            args,
        });
        let line = serde_json::to_string(&frame)?;
        agent
            .stdin_tx
            .send(line)
            .await
            .map_err(|_| AgentError::Channel)?;

        match result_rx.await {
            Ok(res) => res,
            Err(_) => Err(AgentError::AgentGone),
        }
    }

    /// Same as [`call`] but also returns a receiver of streamed event payloads
    /// emitted before the final result. Useful for long ops like `fs.walk`.
    pub async fn call_streaming(
        &self,
        distro: &str,
        op: &str,
        args: Value,
    ) -> Result<(mpsc::UnboundedReceiver<Value>, Result<Value, AgentError>), AgentError> {
        let agent = self.ensure_agent(distro).await?;
        let (result_tx, result_rx) = oneshot::channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let id = new_request_id();

        {
            let mut pending = agent.pending.lock().await;
            pending.insert(
                id.clone(),
                PendingCall {
                    events: Some(events_tx),
                    result: result_tx,
                },
            );
        }

        let frame = Frame::Request(RequestFrame {
            id: id.clone(),
            op: op.to_owned(),
            args,
        });
        let line = serde_json::to_string(&frame)?;
        agent
            .stdin_tx
            .send(line)
            .await
            .map_err(|_| AgentError::Channel)?;

        let result = match result_rx.await {
            Ok(res) => res,
            Err(_) => Err(AgentError::AgentGone),
        };
        Ok((events_rx, result))
    }

    /// Start a long-lived streaming op (e.g. `fs.watch`) that never sends a
    /// final Result. Returns the event receiver plus the request id, which
    /// doubles as the cancellation handle. Unlike [`call_streaming`], this does
    /// NOT await a result — the request stays open until the caller cancels it
    /// or the agent dies (EOF drops the pending entry, closing the receiver).
    ///
    /// [`call_streaming`]: Self::call_streaming
    pub async fn call_open_stream(
        &self,
        distro: &str,
        op: &str,
        args: Value,
    ) -> Result<(mpsc::UnboundedReceiver<Value>, String), AgentError> {
        let agent = self.ensure_agent(distro).await?;
        // The result channel is kept only so the pending entry is well-formed;
        // a watch never resolves it. If the agent dies, the EOF drain fires it
        // (into a dropped receiver) and drops `events_tx`, closing `events_rx`.
        let (result_tx, _result_rx) = oneshot::channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let id = new_request_id();

        {
            let mut pending = agent.pending.lock().await;
            pending.insert(
                id.clone(),
                PendingCall {
                    events: Some(events_tx),
                    result: result_tx,
                },
            );
        }

        let frame = Frame::Request(RequestFrame {
            id: id.clone(),
            op: op.to_owned(),
            args,
        });
        let line = serde_json::to_string(&frame)?;
        agent
            .stdin_tx
            .send(line)
            .await
            .map_err(|_| AgentError::Channel)?;

        Ok((events_rx, id))
    }

    async fn ensure_agent(&self, distro: &str) -> Result<Arc<AgentHandle>, AgentError> {
        {
            let agents = self.agents.lock().await;
            if let Some(a) = agents.get(distro) {
                return Ok(a.clone());
            }
        }

        self.deploy_if_needed(distro).await?;
        let handle = self.spawn_agent(distro).await?;
        let handle = Arc::new(handle);

        let mut agents = self.agents.lock().await;
        agents.insert(distro.to_owned(), handle.clone());
        Ok(handle)
    }

    async fn deploy_if_needed(&self, distro: &str) -> Result<(), AgentError> {
        if !self.host_agent_path.exists() {
            return Err(AgentError::BinaryMissing {
                path: self.host_agent_path.display().to_string(),
            });
        }

        // wsl.exe mangles forwarded args (eats `\` as escape, sometimes
        // collapses other characters depending on version). Earlier
        // attempts — forward-slash workaround, WSLENV /p, base64 in args —
        // all hit edge cases on at least one user's setup. The robust
        // path: stream the entire shell script + payload over stdin to
        // `bash`, so no script bytes ever flow through wsl.exe's arg
        // parser. Backticks, single quotes, backslashes, newlines all
        // survive intact.
        let host_path = self.host_agent_path.to_string_lossy().into_owned();
        // The host path is embedded in a heredoc body. Heredoc bodies
        // expand `$VAR`, backticks and `\`, so quote the delimiter (`'EOF'`)
        // to disable expansion entirely — the path then survives byte-for-byte.
        // Reject paths that contain the delimiter to keep heredoc parsing
        // unambiguous (Windows paths never contain `\nOXYRIS_EOF\n` in practice).
        let delim = "OXYRIS_EOF_8a4f3";
        if host_path.contains(delim) {
            return Err(AgentError::DeployFailed {
                stage: "validate_host_path",
                stderr: format!("host path contains reserved delimiter: {host_path}"),
            });
        }
        let script = format!(
            "set -e\n\
             win_path=$(cat <<'{delim}'\n\
             {host_path}\n\
             {delim}\n\
             )\n\
             # `cat <<\\'...\\'` preserves the path's leading whitespace from\n\
             # our line-continuation indentation, so trim it.\n\
             win_path=${{win_path#\"${{win_path%%[![:space:]]*}}\"}}\n\
             win_path=${{win_path%\"${{win_path##*[![:space:]]}}\"}}\n\
             src=$(wslpath -u \"$win_path\" 2>/dev/null || true)\n\
             if [ -z \"$src\" ]; then\n\
                 echo \"wslpath -u returned empty for: [$win_path]\" >&2\n\
                 exit 1\n\
             fi\n\
             if [ ! -f \"$src\" ]; then\n\
                 echo \"agent binary not visible from inside the distro at $src (host path: $win_path)\" >&2\n\
                 exit 1\n\
             fi\n\
             mkdir -p ~/.oxyris/bin\n\
             dst=~/.oxyris/bin/oxyris-agent\n\
             if [ ! -f \"$dst\" ] || [ \"$src\" -nt \"$dst\" ]; then\n\
                 # Never `cp` onto $dst directly: an agent from a previous run\n\
                 # may still be executing it, and Linux answers ETXTBSY\n\
                 # (\"Text file busy\") for writes to a mapped executable.\n\
                 # Copy to a sibling temp, then `mv` — rename(2) unlinks the old\n\
                 # inode, which the running process keeps open until it exits.\n\
                 tmp=\"$dst.$$.tmp\"\n\
                 trap 'rm -f \"$tmp\"' EXIT\n\
                 cp \"$src\" \"$tmp\"\n\
                 chmod +x \"$tmp\"\n\
                 mv -f \"$tmp\" \"$dst\"\n\
             fi\n"
        );

        // `bash -s` reads the script body from stdin. We use bash specifically
        // because `sh` on Alpine is busybox and lacks the `${var#pattern}`
        // shapes used above; bash is universally available on Ubuntu/Debian/
        // Fedora/Arch and on Alpine after `apk add bash`.
        let mut child = Command::new("wsl.exe")
            .args(["-d", distro, "--", "bash", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .hide_console()
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script.as_bytes()).await?;
            stdin.shutdown().await?;
        }
        let out = child.wait_with_output().await?;
        if !out.status.success() {
            return Err(AgentError::DeployFailed {
                stage: "copy_and_chmod",
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        Ok(())
    }

    async fn spawn_agent(&self, distro: &str) -> Result<AgentHandle, AgentError> {
        let mut child = Command::new("wsl.exe")
            .args([
                "-d",
                distro,
                "--",
                "sh",
                "-lc",
                "~/.oxyris/bin/oxyris-agent",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .hide_console()
            .spawn()?;

        let stdin = child.stdin.take().ok_or(AgentError::AgentGone)?;
        let stdout = child.stdout.take().ok_or(AgentError::AgentGone)?;
        let stderr = child.stderr.take().ok_or(AgentError::AgentGone)?;

        let pending: Arc<Mutex<HashMap<String, PendingCall>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Writer task drains the outgoing channel into the agent's stdin.
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(32);
        tokio::spawn(async move {
            let mut stdin: ChildStdin = stdin;
            while let Some(mut line) = stdin_rx.recv().await {
                line.push('\n');
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // Stderr drain — agent uses stderr for logs.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(target = "oxyris_agent", "{line}");
            }
        });

        // Stdout reader: parse NDJSON frames, dispatch to pending entries.
        let pending_reader = pending.clone();
        let reader_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let frame: Frame = match serde_json::from_str(&line) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, raw = %line, "agent emitted malformed frame");
                        continue;
                    }
                };
                route_frame(&pending_reader, frame).await;
            }
            // EOF — cancel all pending calls.
            let mut pending = pending_reader.lock().await;
            for (_, call) in pending.drain() {
                let _ = call.result.send(Err(AgentError::AgentGone));
            }
        });

        Ok(AgentHandle {
            stdin_tx,
            pending,
            _child: child,
            _task: reader_task,
        })
    }
}

async fn route_frame(pending: &Arc<Mutex<HashMap<String, PendingCall>>>, frame: Frame) {
    match frame {
        Frame::Event(ev) => {
            let pending = pending.lock().await;
            if let Some(entry) = pending.get(&ev.request_id)
                && let Some(sender) = entry.events.as_ref()
            {
                let _ = sender.send(ev.data);
            }
        }
        Frame::Result(res) => {
            let mut pending = pending.lock().await;
            if let Some(entry) = pending.remove(&res.request_id) {
                let _ = entry.result.send(Ok(res.data));
            }
        }
        Frame::Error(err) => {
            let mut pending = pending.lock().await;
            if let Some(entry) = pending.remove(&err.request_id) {
                let _ = entry.result.send(Err(AgentError::Remote {
                    code: err.code,
                    message: err.message,
                }));
            }
        }
        Frame::Request(_) => {
            // Agents never initiate; swallow.
        }
    }
}

fn new_request_id() -> String {
    format!("r-{}", uuid::Uuid::now_v7())
}

/// Walk up from the current executable (e.g. `target/debug/oxyris-desktop.exe`)
/// looking for a `dist/agent/oxyris-agent` the build script would have placed
/// at the workspace root. Returns the first hit, or `None` if we leave the
/// filesystem without finding it.
fn find_dev_agent_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..6 {
        let candidate = dir.join("dist").join("agent").join("oxyris-agent");
        if candidate.exists() {
            return Some(candidate);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    None
}
