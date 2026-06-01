//! Claude provider adapter — spawns the `claude` CLI and pumps a stream-json
//! conversation through [`oxyris_provider::Provider`] channels.

use std::process::Stdio;

use oxyris_core::Environment;
use oxyris_procutil::HideConsole;
use oxyris_provider::{
    Provider, ProviderCommand, ProviderError, ProviderEvent, ProviderSession, SessionOptions,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use crate::protocol::{StreamEvent, parse_stream_line};

pub struct ClaudeProvider;

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self
    }
}

impl Provider for ClaudeProvider {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        "Claude"
    }

    fn start_session(&self, opts: SessionOptions) -> Result<ProviderSession, ProviderError> {
        // Modes that route tool approvals over stdio (see `build_command`)
        // need the control-protocol handshake in the writer task.
        let permission_stdio = matches!(
            opts.runtime,
            oxyris_provider::RuntimeMode::Supervised | oxyris_provider::RuntimeMode::AcceptEdits
        );
        let mut cmd = build_command(&opts)?;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| ProviderError::Spawn(e.to_string()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Spawn("stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Spawn("stdout unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProviderError::Spawn("stderr unavailable".into()))?;

        let (events_tx, events_rx) = mpsc::unbounded_channel::<ProviderEvent>();
        let (commands_tx, mut commands_rx) = mpsc::unbounded_channel::<ProviderCommand>();
        // One-shot kill channel — fired by the writer task when it sees an
        // Interrupt or Stop command. The reaper races this against the
        // child's natural exit, so killing the process is non-blocking from
        // the IPC handler's perspective.
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let mut kill_tx = Some(kill_tx);

        // stderr logger — claude writes diagnostics here.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(target = "claude.cli.stderr", "{line}");
            }
        });

        // stdout → ProviderEvent fan-out.
        let evt = events_tx.clone();
        let active_turn: std::sync::Arc<std::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let turn_ref = active_turn.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(event) = parse_stream_line(&line) {
                    translate_and_emit(&evt, &turn_ref, event);
                }
            }
            let _ = evt.send(ProviderEvent::SessionEnded);
        });

        // Command writer — serializes ProviderCommands into Claude's
        // input stream-json shape and writes them to the child's stdin.
        // Holds the kill_tx so Interrupt/Stop can ask the reaper to kill
        // the child, which is the only reliable way to stop streaming —
        // Claude CLI's stream-json input has no "cancel" frame.
        let turn_ref = active_turn.clone();
        tokio::spawn(async move {
            let mut stdin = stdin;
            // When permission prompts are routed over stdio, claude expects the
            // control protocol to be live: announce it with one `initialize`
            // control_request before any user input. The CLI's control_response
            // is parsed as Unknown and dropped.
            if permission_stdio {
                let init = serde_json::json!({
                    "type": "control_request",
                    "request_id": "oxyris-init-1",
                    "request": { "subtype": "initialize", "hooks": serde_json::Value::Null }
                });
                let _ = write_line(&mut stdin, &init).await;
            }
            while let Some(cmd) = commands_rx.recv().await {
                match cmd {
                    ProviderCommand::SendMessage { turn_id, text } => {
                        if let Ok(mut guard) = turn_ref.lock() {
                            *guard = Some(turn_id);
                        }
                        let line = serde_json::json!({
                            "type": "user",
                            "message": {
                                "role": "user",
                                "content": [ { "type": "text", "text": text } ]
                            }
                        });
                        if write_line(&mut stdin, &line).await.is_err() {
                            break;
                        }
                    }
                    ProviderCommand::Interrupt | ProviderCommand::Stop => {
                        // Fire the kill signal exactly once — subsequent
                        // Interrupts on a dying session are no-ops.
                        if let Some(tx) = kill_tx.take() {
                            let _ = tx.send(());
                        }
                        // Stop reading commands; the session is going down.
                        break;
                    }
                    ProviderCommand::ApproveToolUse { request_id } => {
                        // Allow with no `updatedInput` → claude runs the tool
                        // with the original input it asked about.
                        let line = serde_json::json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "success",
                                "request_id": request_id,
                                "response": { "behavior": "allow" }
                            }
                        });
                        if write_line(&mut stdin, &line).await.is_err() {
                            break;
                        }
                    }
                    ProviderCommand::RejectToolUse {
                        request_id,
                        message,
                    } => {
                        let line = serde_json::json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "success",
                                "request_id": request_id,
                                "response": { "behavior": "deny", "message": message }
                            }
                        });
                        if write_line(&mut stdin, &line).await.is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = stdin.shutdown().await;
        });

        // Reaper — races the child's natural exit against an external kill
        // signal from the writer. Whichever fires first wins; either way the
        // child is reaped so it doesn't zombie.
        tokio::spawn(async move {
            tokio::select! {
                _ = kill_rx => {
                    tracing::debug!("claude: interrupt requested, killing child");
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
                result = child.wait() => {
                    tracing::debug!(?result, "claude: child exited naturally");
                }
            }
        });

        Ok(ProviderSession {
            commands: commands_tx,
            events: events_rx,
            provider_session_id: None,
        })
    }
}

async fn write_line(
    stdin: &mut tokio::process::ChildStdin,
    value: &serde_json::Value,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(value).unwrap_or_default();
    bytes.push(b'\n');
    stdin.write_all(&bytes).await?;
    stdin.flush().await
}

fn translate_and_emit(
    tx: &mpsc::UnboundedSender<ProviderEvent>,
    turn_ref: &std::sync::Arc<std::sync::Mutex<Option<String>>>,
    event: StreamEvent,
) {
    let turn_id = turn_ref
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default();
    match event {
        StreamEvent::System { session_id, model } => {
            let _ = tx.send(ProviderEvent::SessionReady {
                provider_session_id: session_id,
                model: model.unwrap_or_else(|| "unknown".into()),
            });
        }
        StreamEvent::Assistant { blocks } => {
            for block in blocks {
                let _ = tx.send(ProviderEvent::AssistantBlock {
                    turn_id: turn_id.clone(),
                    block,
                });
            }
        }
        StreamEvent::ToolResult {
            tool_use_id,
            output,
            is_error,
        } => {
            let _ = tx.send(ProviderEvent::AssistantBlock {
                turn_id: turn_id.clone(),
                block: oxyris_provider::AssistantBlock::ToolResult {
                    tool_use_id,
                    output,
                    is_error,
                },
            });
        }
        StreamEvent::Result {
            is_error,
            text: _,
            total_cost_usd,
            input_tokens,
            output_tokens,
        } => {
            if is_error {
                let _ = tx.send(ProviderEvent::TurnFailed {
                    turn_id,
                    message: "claude reported is_error=true".into(),
                });
            } else {
                let _ = tx.send(ProviderEvent::TurnCompleted {
                    turn_id,
                    total_cost_usd,
                    input_tokens,
                    output_tokens,
                });
            }
        }
        StreamEvent::CanUseTool {
            request_id,
            tool_use_id,
            tool_name,
            input,
        } => {
            let _ = tx.send(ProviderEvent::ToolApprovalRequested {
                turn_id,
                request_id,
                tool_use_id,
                tool_name,
                input,
            });
        }
        StreamEvent::Unknown(_) => {}
    }
}

fn build_command(opts: &SessionOptions) -> Result<Command, ProviderError> {
    // Core CLI flags — stream-json both directions so we don't have to parse
    // human output. `--model` is optional: an empty `opts.model` lets Claude
    // pick its own default (same behaviour as running `claude` bare).
    let mut claude_args: Vec<&str> = vec![
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ];
    if !opts.model.trim().is_empty() {
        claude_args.push("--model");
        claude_args.push(opts.model.as_str());
    }

    let claude_args = claude_args; // freeze
    let mut cmd = match &opts.environment {
        Environment::Local => {
            // Claude is usually installed as `claude.cmd` (npm shim) rather
            // than `claude.exe`, so `Command::new("claude.exe")` misses it
            // even when PATH has claude. `which` resolves PATH + PATHEXT.
            // If the target is a batch file, CreateProcess can't run it
            // directly — we forward through `cmd.exe /C`.
            let full = which::which("claude")
                .or_else(|_| which::which("claude.cmd"))
                .or_else(|_| which::which("claude.exe"))
                .map_err(|e| {
                    ProviderError::Spawn(format!(
                        "claude not found on PATH (checked `claude`, `claude.cmd`, `claude.exe`): {e}"
                    ))
                })?;
            let is_batch = full
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "cmd" | "bat"))
                .unwrap_or(false);
            let mut cmd = if is_batch {
                let mut c = Command::new("cmd.exe");
                c.arg("/C");
                c.arg(&full);
                c
            } else {
                Command::new(&full)
            };
            cmd.args(claude_args);
            cmd.current_dir(&opts.cwd);
            cmd
        }
        Environment::Wsl { distro } => {
            // Spawn `claude` inside the distro via wsl.exe. The agent isn't
            // used here because stdio proxies cleanly through wsl.exe — the
            // backend still owns the child, which matches our supervision
            // model for turn interrupts.
            let cwd = opts.cwd.clone();
            let script = format!(
                "cd {cwd:?} && exec claude {args}",
                args = claude_args
                    .iter()
                    .map(|a| shell_escape(a))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            let mut cmd = Command::new("wsl.exe");
            cmd.args(["-d", distro, "--", "sh", "-lc", &script]);
            cmd
        }
    };

    // Runtime mode → Claude policy flags. We use the public switches exposed
    // via `--permission-mode` (claude 2.x+). Anything unknown falls back to
    // the CLI default.
    // `--permission-prompt-tool stdio` routes "would prompt" decisions back to
    // us as `can_use_tool` control_requests instead of auto-denying in
    // non-interactive mode. Enabled for the modes that ask (Supervised always;
    // AcceptEdits for non-edit tools). FullAccess bypasses; Plan keeps the
    // CLI's own plan-approval flow.
    match opts.runtime {
        oxyris_provider::RuntimeMode::FullAccess => {
            cmd.args(["--permission-mode", "bypassPermissions"]);
        }
        oxyris_provider::RuntimeMode::AcceptEdits => {
            cmd.args([
                "--permission-mode",
                "acceptEdits",
                "--permission-prompt-tool",
                "stdio",
            ]);
        }
        oxyris_provider::RuntimeMode::Supervised => {
            cmd.args([
                "--permission-mode",
                "default",
                "--permission-prompt-tool",
                "stdio",
            ]);
        }
        oxyris_provider::RuntimeMode::Plan => {
            cmd.args(["--permission-mode", "plan"]);
        }
    }

    if let Some(prompt) = opts.system_prompt.as_ref() {
        cmd.args(["--append-system-prompt", prompt]);
    }

    if let Some(resume_id) = opts.resume_session_id.as_ref() {
        cmd.args(["--resume", resume_id]);
    }

    if let Some(mcp_path) = opts.mcp_config_path.as_ref() {
        cmd.args(["--mcp-config", mcp_path]);
    }

    cmd.hide_console();
    Ok(cmd)
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
