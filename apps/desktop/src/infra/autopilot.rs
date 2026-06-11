//! Auto-pilot integration — wires the pure `Supervisor` decision core
//! (`oxyris-supervisor`) to live sessions.
//!
//! Flow: the PTY reader detects a pure-signal and pushes a [`PureSignalNotice`]
//! to this manager's channel. For an *engaged* session the manager builds the
//! transcript context from the PTY scrollback, runs [`Autopilot::step`]
//! (denylist → Supervisor → budget), and carries out the resulting [`Action`]
//! by writing to the PTY stdin. Every step emits `session:<id>:autopilot` so the
//! UI mini-log can show what the pilot did; a [`Action::Halt`] disengages.
//!
//! Two Supervisor backends ship:
//! - [`OpenAiCompatSupervisor`] — any OpenAI-compatible `/chat/completions`
//!   endpoint (OpenAI, OpenRouter, Groq, Ollama, …) = the "multi-model" option.
//! - [`ClaudeCliSupervisor`] — a headless `claude -p` acting as judge.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oxyris_core::AggregateId;
use oxyris_supervisor::{
    Action, Autopilot, AutopilotContext, Decision, HaltReason, Mission, PendingKind, Supervisor,
    SupervisorError, SupervisorKind, TranscriptView,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, mpsc::UnboundedReceiver};

use crate::infra::pty::{PtySupervisor, PureSignalNotice};
use crate::infra::pure_signals::PureSignal;

/// How much PTY scrollback (chars) to feed the Supervisor as context.
const CONTEXT_CHARS: usize = 4000;

/// Config chosen in the auto-pilot panel, passed through `autopilot_engage`.
#[derive(Debug, Clone)]
pub enum SupervisorConfig {
    MultiModel {
        base_url: String,
        model: String,
        api_key: Option<String>,
    },
    Claude {
        model: Option<String>,
    },
}

impl SupervisorConfig {
    fn build(self) -> Box<dyn Supervisor> {
        match self {
            SupervisorConfig::MultiModel {
                base_url,
                model,
                api_key,
            } => Box::new(OpenAiCompatSupervisor::new(base_url, model, api_key)),
            SupervisorConfig::Claude { model } => Box::new(ClaudeCliSupervisor::new(model)),
        }
    }
}

struct EngagedSession {
    term_id: String,
    cwd: String,
    mission: Mission,
    /// Serializes steps for one session — the Supervisor call is async and we
    /// must not interleave two steps on the same conversation.
    pilot: Mutex<Autopilot>,
}

/// What the frontend gets on `session:<id>:autopilot` for its mini-log.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AutopilotEvent {
    /// A step started — the Supervisor is being consulted. Emitted before the
    /// (possibly slow) LLM call so the UI can show the pilot is reacting; cleared
    /// by whichever terminal event below follows.
    Thinking,
    Approved,
    Rejected {
        reason: String,
    },
    Replied {
        text: String,
    },
    Halted {
        reason: String,
    },
    Error {
        message: String,
    },
}

pub struct AutopilotManager {
    pty: Arc<PtySupervisor>,
    app: AppHandle,
    engaged: Mutex<HashMap<AggregateId, Arc<EngagedSession>>>,
}

impl AutopilotManager {
    pub fn new(pty: Arc<PtySupervisor>, app: AppHandle) -> Self {
        Self {
            pty,
            app,
            engaged: Mutex::new(HashMap::new()),
        }
    }

    /// Engage the pilot for a session. Resolves the session's claude PTY (must
    /// already be spawned) and stores the mission + a fresh [`Autopilot`].
    pub async fn engage(
        self: &Arc<Self>,
        session_id: AggregateId,
        mission: String,
        config: SupervisorConfig,
        max_turns: Option<u32>,
    ) -> Result<(), String> {
        let mission = Mission::new(mission);
        if mission.is_empty() {
            return Err("mission is empty".into());
        }
        let term = self
            .pty
            .list_for_session(session_id)
            .into_iter()
            .find(|t| matches!(t.kind, crate::infra::pty::TerminalKind::Claude))
            .ok_or_else(|| "no pure (claude) terminal for this session".to_owned())?;

        let pilot = Autopilot::new(config.build(), max_turns);
        let engaged = Arc::new(EngagedSession {
            term_id: term.id,
            cwd: term.cwd,
            mission,
            pilot: Mutex::new(pilot),
        });
        self.engaged
            .lock()
            .await
            .insert(session_id, engaged.clone());

        // Kick the first decision. The pilot is otherwise reactive — it acts only
        // when a NEW pure-signal arrives. Engaging while claude sits idle (the
        // common case: you set a mission on a quiet session) never produces one,
        // so the run would never start. Treat the current idle state as a
        // `TurnEnded` and let the supervisor send the opening instruction. A short
        // delay lets any in-flight signal settle; the per-session `pilot` mutex
        // serializes this against a real signal racing in.
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let last_output = this
                .pty
                .scrollback_tail(&engaged.term_id, CONTEXT_CHARS)
                .unwrap_or_default();
            this.drive(session_id, &engaged, PendingKind::TurnEnded { last_output })
                .await;
        });
        Ok(())
    }

    pub async fn disengage(&self, session_id: AggregateId) {
        self.engaged.lock().await.remove(&session_id);
    }

    /// Consume the PTY reader's notice channel forever. Each notice for an
    /// engaged session is handled on its own task so a slow Supervisor call
    /// doesn't stall the channel; per-session serialization is enforced by the
    /// session's `pilot` mutex.
    pub async fn run(self: Arc<Self>, mut rx: UnboundedReceiver<PureSignalNotice>) {
        while let Some(notice) = rx.recv().await {
            // Working is a keep-alive only — no decision to make.
            if matches!(notice.signal, PureSignal::Working) {
                continue;
            }
            let Some(session) = self.engaged.lock().await.get(&notice.session_id).cloned() else {
                continue;
            };
            // Ignore signals from a stale PTY (e.g. a respawned claude after a
            // crash) that no longer matches the engaged terminal.
            if notice.terminal_id != session.term_id {
                continue;
            }
            let this = self.clone();
            tokio::spawn(async move {
                this.handle(notice, session).await;
            });
        }
    }

    async fn handle(&self, notice: PureSignalNotice, session: Arc<EngagedSession>) {
        let Some(ask) = self.build_pending(&notice, &session) else {
            return;
        };
        self.drive(notice.session_id, &session, ask).await;
    }

    /// Run one Supervisor step for `ask` and carry out its action. Shared by the
    /// signal-driven path ([`Self::handle`]) and the engage kickoff.
    async fn drive(&self, session_id: AggregateId, session: &EngagedSession, ask: PendingKind) {
        // Signal "reacting now" up front — the Supervisor call can take seconds,
        // and without this the user sees no movement between a prompt appearing
        // and the pilot acting.
        self.emit(session_id, AutopilotEvent::Thinking);
        let ctx = AutopilotContext {
            mission: session.mission.clone(),
            transcript: TranscriptView {
                title: None,
                recent_output: self
                    .pty
                    .scrollback_tail(&session.term_id, CONTEXT_CHARS)
                    .unwrap_or_default(),
            },
            cwd: session.cwd.clone(),
        };

        let action = {
            let mut pilot = session.pilot.lock().await;
            pilot.step(&ctx, &ask).await
        };

        match action {
            Ok(action) => self.act(session_id, session, action).await,
            Err(e) => {
                self.emit(
                    session_id,
                    AutopilotEvent::Error {
                        message: e.to_string(),
                    },
                );
                // A backend failure isn't a reason to keep retrying blindly —
                // hand control back so the user notices.
                self.disengage(session_id).await;
            }
        }
    }

    fn build_pending(
        &self,
        notice: &PureSignalNotice,
        session: &EngagedSession,
    ) -> Option<PendingKind> {
        let tail = self
            .pty
            .scrollback_tail(&session.term_id, CONTEXT_CHARS)
            .unwrap_or_default();
        match notice.signal {
            PureSignal::NeedsInput => Some(PendingKind::Permission {
                request_id: None,
                tool_name: None,
                command: None,
                raw_prompt: tail,
            }),
            PureSignal::TurnEnded => Some(PendingKind::TurnEnded { last_output: tail }),
            PureSignal::Working => None,
        }
    }

    async fn act(&self, session_id: AggregateId, session: &EngagedSession, action: Action) {
        let term = &session.term_id;
        match action {
            Action::Approve => {
                // Accept the highlighted default (option 1 = Yes) with Enter.
                let _ = self.pty.write(term, "\r");
                self.emit(session_id, AutopilotEvent::Approved);
            }
            Action::Reject(reason) => {
                // Esc cancels the menu; then send the reason as a message.
                let _ = self.pty.write(term, "\x1b");
                tokio::time::sleep(Duration::from_millis(80)).await;
                self.submit(term, &reason).await;
                self.emit(session_id, AutopilotEvent::Rejected { reason });
            }
            Action::Reply(text) => {
                self.submit(term, &text).await;
                self.emit(session_id, AutopilotEvent::Replied { text });
            }
            Action::Halt(reason) => {
                self.disengage(session_id).await;
                self.emit(
                    session_id,
                    AutopilotEvent::Halted {
                        reason: halt_reason_str(&reason),
                    },
                );
            }
        }
    }

    /// Send text then a separate carriage return — claude's TUI has paste-burst
    /// detection, so `text\r` in one write becomes a literal newline instead of
    /// submitting. Mirrors the frontend `sendToPty`.
    async fn submit(&self, term_id: &str, text: &str) {
        let _ = self.pty.write(term_id, text);
        tokio::time::sleep(Duration::from_millis(60)).await;
        let _ = self.pty.write(term_id, "\r");
    }

    fn emit(&self, session_id: AggregateId, event: AutopilotEvent) {
        let _ = self
            .app
            .emit(&format!("session:{session_id}:autopilot"), event);
    }
}

fn halt_reason_str(reason: &HaltReason) -> String {
    match reason {
        HaltReason::Done(s) => format!("done: {s}"),
        HaltReason::Escalated(s) => format!("escalated: {s}"),
        HaltReason::Denylisted(s) => format!("blocked (denylist): {s}"),
        HaltReason::Looping => "stopped: detected a loop".into(),
        HaltReason::BudgetExhausted => "stopped: turn budget exhausted".into(),
    }
}

// ── Supervisor backends ──────────────────────────────────────────────────────

/// System instruction shared by both backends. Pins the output contract to the
/// JSON shape `serde` parses into [`Decision`] (`#[serde(tag = "decision")]`).
const SYSTEM_PROMPT: &str = "You are an autonomous supervisor driving a Claude Code coding session toward a stated mission, acting in place of the user.\n\nRespond with ONLY a single JSON object, no prose, no markdown fences. It must be exactly one of:\n{\"decision\":\"approve\"}\n{\"decision\":\"reject\",\"reason\":\"<why>\"}\n{\"decision\":\"reply\",\"text\":\"<message to send>\"}\n{\"decision\":\"done\",\"summary\":\"<what was accomplished>\"}\n{\"decision\":\"escalate\",\"why\":\"<why a human is needed>\"}\n\nGuidance: approve tool uses that safely advance the mission; reject unsafe or off-mission ones with a reason. When Claude asks a question, reply with the answer that best serves the mission. When Claude has finished a turn, decide whether the mission is complete (done) or send the next concrete instruction (reply). Escalate when you are unsure or the situation looks risky.";

fn build_user_prompt(ctx: &AutopilotContext, ask: &PendingKind) -> String {
    let pending = match ask {
        PendingKind::Permission { raw_prompt, .. } => {
            format!(
                "Claude is waiting for input (a tool approval or a question). On-screen prompt:\n{raw_prompt}"
            )
        }
        PendingKind::TurnEnded { last_output } => {
            format!(
                "Claude finished a turn and is idle. Decide whether the mission is complete or what the next instruction should be.\nRecent output:\n{last_output}"
            )
        }
    };
    format!(
        "# Mission\n{}\n\n# Working directory\n{}\n\n# Recent session output\n{}\n\n# Pending\n{}",
        ctx.mission.text.trim(),
        ctx.cwd,
        ctx.transcript.recent_output.trim(),
        pending,
    )
}

/// Extract a `Decision` from a model reply that may be wrapped in prose or code
/// fences. Finds the outermost `{ … }` and parses it.
fn parse_decision(content: &str) -> Result<Decision, SupervisorError> {
    let start = content.find('{');
    let end = content.rfind('}');
    let json = match (start, end) {
        (Some(s), Some(e)) if e >= s => &content[s..=e],
        _ => {
            return Err(SupervisorError::InvalidDecision(format!(
                "no JSON object in reply: {content}"
            )));
        }
    };
    serde_json::from_str::<Decision>(json)
        .map_err(|e| SupervisorError::InvalidDecision(format!("{e}: {json}")))
}

/// Supervisor backed by any OpenAI-compatible `/chat/completions` endpoint.
pub struct OpenAiCompatSupervisor {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAiCompatSupervisor {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
            api_key,
        }
    }
}

#[async_trait]
impl Supervisor for OpenAiCompatSupervisor {
    fn id(&self) -> &'static str {
        "multi-model"
    }

    async fn decide(
        &self,
        ctx: &AutopilotContext,
        ask: &PendingKind,
    ) -> Result<Decision, SupervisorError> {
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": build_user_prompt(ctx, ask) },
            ],
        });
        let mut req = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| SupervisorError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(SupervisorError::Backend(format!("{status}: {text}")));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| SupervisorError::Backend(e.to_string()))?;
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| SupervisorError::Backend("no choices[0].message.content".into()))?;
        parse_decision(content)
    }
}

/// Supervisor backed by a headless `claude -p` invocation acting as judge.
pub struct ClaudeCliSupervisor {
    model: Option<String>,
}

impl ClaudeCliSupervisor {
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }
}

#[async_trait]
impl Supervisor for ClaudeCliSupervisor {
    fn id(&self) -> &'static str {
        "claude-cli"
    }

    async fn decide(
        &self,
        ctx: &AutopilotContext,
        ask: &PendingKind,
    ) -> Result<Decision, SupervisorError> {
        let prompt = build_user_prompt(ctx, ask);
        let model = self.model.clone();
        let output =
            tokio::task::spawn_blocking(move || run_claude_judge(&prompt, model.as_deref()))
                .await
                .map_err(|e| SupervisorError::Backend(format!("join: {e}")))??;
        parse_decision(&output)
    }
}

/// Blocking `claude -p` call. Resolves the binary like the rest of the app
/// (npm shim is usually `claude.cmd`, launched via `cmd.exe /C`).
fn run_claude_judge(prompt: &str, model: Option<&str>) -> Result<String, SupervisorError> {
    use oxyris_procutil::HideConsole;
    use std::io::Write;
    use std::process::{Command, Stdio};

    let full = which::which("claude")
        .or_else(|_| which::which("claude.cmd"))
        .or_else(|_| which::which("claude.exe"))
        .map_err(|e| SupervisorError::NotConfigured(format!("claude not found: {e}")))?;
    let is_batch = full
        .extension()
        .and_then(|x| x.to_str())
        .map(|x| matches!(x.to_ascii_lowercase().as_str(), "cmd" | "bat"))
        .unwrap_or(false);
    let mut cmd = if is_batch {
        let mut c = Command::new("cmd.exe");
        c.arg("/C");
        c.arg(full.as_os_str());
        c
    } else {
        Command::new(full.as_os_str())
    };
    // Feed the user prompt over stdin, NOT as an argv arg. With the scrollback
    // context the prompt easily exceeds the Windows command-line limit (cmd.exe
    // caps at ~8 KB) → "Linha de comando muito longa" / exit 1. `claude -p` with
    // no positional reads the prompt from stdin. The system prompt stays an arg
    // (it's small and fixed).
    cmd.arg("-p");
    cmd.arg("--append-system-prompt").arg(SYSTEM_PROMPT);
    if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
        cmd.arg("--model").arg(m);
    }
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .hide_console()
        .spawn()
        .map_err(|e| SupervisorError::Backend(e.to_string()))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| SupervisorError::Backend("failed to open claude stdin".into()))?;
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| SupervisorError::Backend(e.to_string()))?;
        // Dropping `stdin` here closes the pipe → claude sees EOF and starts.
    }
    let out = child
        .wait_with_output()
        .map_err(|e| SupervisorError::Backend(e.to_string()))?;
    if !out.status.success() {
        return Err(SupervisorError::Backend(format!(
            "claude exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Map the frontend's supervisor selector + fields to a [`SupervisorConfig`].
pub fn config_from_parts(
    kind: SupervisorKind,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
) -> Result<SupervisorConfig, String> {
    match kind {
        SupervisorKind::MultiModel => {
            let base_url = base_url
                .filter(|u| !u.trim().is_empty())
                .ok_or_else(|| "multi-model supervisor needs a base URL".to_owned())?;
            let model = model
                .filter(|m| !m.trim().is_empty())
                .ok_or_else(|| "multi-model supervisor needs a model".to_owned())?;
            Ok(SupervisorConfig::MultiModel {
                base_url,
                model,
                api_key: api_key.filter(|k| !k.trim().is_empty()),
            })
        }
        SupervisorKind::Claude => Ok(SupervisorConfig::Claude {
            model: model.filter(|m| !m.trim().is_empty()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decision_handles_fenced_json() {
        let d = parse_decision("```json\n{\"decision\":\"approve\"}\n```").unwrap();
        assert!(matches!(d, Decision::Approve));
    }

    #[test]
    fn parse_decision_handles_prose_wrapped() {
        let d = parse_decision("Sure. {\"decision\":\"reply\",\"text\":\"go on\"} done").unwrap();
        assert!(matches!(d, Decision::Reply { text } if text == "go on"));
    }

    #[test]
    fn parse_decision_rejects_garbage() {
        assert!(parse_decision("no json here").is_err());
    }

    #[test]
    fn config_multimodel_requires_base_url() {
        let r = config_from_parts(SupervisorKind::MultiModel, Some("gpt".into()), None, None);
        assert!(r.is_err());
    }

    #[test]
    fn config_claude_is_lenient() {
        let r = config_from_parts(SupervisorKind::Claude, None, None, None);
        assert!(matches!(r, Ok(SupervisorConfig::Claude { model: None })));
    }
}
