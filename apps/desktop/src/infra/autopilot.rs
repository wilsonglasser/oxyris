//! Auto-pilot integration — wires the pure `Supervisor` decision core
//! (`oxyris-supervisor`) to live sessions.
//!
//! Flow: per engaged session a **state-driven watchdog** ([`AutopilotManager::watchdog`])
//! polls the claude PTY's ground-truth turn state ([`PtySupervisor::pure_state`])
//! plus a fingerprint of its scrollback. When the session has been *idle and
//! stable* (settled or a menu waiting) for [`RESPOND_DEBOUNCE_MS`], and that exact
//! screen hasn't been acted on before, it runs one [`Autopilot::step`] (denylist →
//! Supervisor → budget) and carries out the [`Action`] by writing to the PTY.
//!
//! Why a watchdog and not raw marker edges: the TUI emits *several* turn-end
//! markers per turn (poll + "Worked for…" + recap), it can settle by silence with
//! no marker at all, and a submitted Enter can be swallowed by paste-burst
//! detection. Reacting to each marker double-fired the reply; relying only on
//! markers stalled the pilot when one was missed. Polling ground-truth state with
//! a debounce + per-screen dedup fixes both, and an explicit *await-start* phase
//! retries a swallowed Enter (idempotently) instead of re-asking the Supervisor.
//! The [`PureSignalNotice`] channel is kept only as a low-latency *wake nudge* so
//! the watchdog re-evaluates promptly instead of waiting a full poll tick.
//!
//! Two Supervisor backends ship:
//! - [`OpenAiCompatSupervisor`] — any OpenAI-compatible `/chat/completions`
//!   endpoint (OpenAI, OpenRouter, Groq, Ollama, …) = the "multi-model" option.
//! - [`ClaudeCliSupervisor`] — a headless `claude -p` acting as judge.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use oxyris_core::AggregateId;
use oxyris_supervisor::{
    Action, Autopilot, AutopilotContext, HaltReason, Mission, PendingKind, Supervisor,
    SupervisorError, SupervisorKind, TranscriptView, Verdict,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Notify, mpsc::UnboundedReceiver};

use crate::infra::pty::{PtySupervisor, PureSignalNotice};
use crate::infra::pure_signals::PureSignal;

/// How much PTY scrollback (chars) to feed the Supervisor as context.
const CONTEXT_CHARS: usize = 4000;

/// Watchdog poll cadence (ms). A wake nudge from a marker signal can fire a tick
/// sooner; this is the safety-net interval that catches silence-based settles.
const POLL_MS: u64 = 350;

/// How long the session must stay idle **and** show the same screen before the
/// pilot acts. Wider than the TUI's mid-turn settle blips so a transient idle
/// frame can't trigger a premature (or duplicate) response.
const RESPOND_DEBOUNCE_MS: u64 = 1000;

/// After the pilot submits, how long to wait for claude to start the turn (go
/// busy) before assuming the Enter was swallowed and re-sending it.
const START_TIMEOUT_MS: u64 = 1300;

/// Max Enter re-sends for one swallowed submit before handing back to the human.
const MAX_SUBMIT_RETRIES: u32 = 3;

/// Gap (ms) between writing the reply text and the submitting Enter. claude's
/// paste-burst detection folds `text\r` in one write (or too close together)
/// into a literal newline instead of a submit, so the Enter is a separate write
/// after this pause. 60 ms was on the edge and intermittently swallowed.
const SUBMIT_GAP_MS: u64 = 120;

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
    session_id: AggregateId,
    term_id: String,
    cwd: String,
    mission: Mission,
    /// Serializes steps for one session — the Supervisor call is async and we
    /// must not interleave two steps on the same conversation.
    pilot: Mutex<Autopilot>,
    /// Low-latency nudge: a marker signal pokes this so the watchdog re-evaluates
    /// without waiting a full [`POLL_MS`] tick. Also poked on disengage to break
    /// the watchdog out of its wait promptly.
    wake: Notify,
}

/// Where the per-session watchdog is in its act → confirm loop.
enum Phase {
    /// Watching for a settled, stable, not-yet-acted screen to respond to.
    Observe,
    /// We submitted and are waiting for claude to actually start the turn (go
    /// busy). If it never does, the Enter was likely swallowed — re-send it.
    AwaitingStart { since: Instant, retries: u32 },
}

/// What the frontend gets on `session:<id>:autopilot` for its mini-log.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AutopilotEvent {
    /// A step started — the Supervisor is being consulted. Emitted before the
    /// (possibly slow) LLM call so the UI can show the pilot is reacting; cleared
    /// by whichever terminal event below follows.
    Thinking,
    /// The Supervisor's one-line rationale for the decision it just made — the
    /// pilot's surfaced "thinking", shown in the auto-pilot button tooltip.
    /// Emitted right after the decision, before the action event.
    Reasoning {
        text: String,
    },
    Approved,
    Rejected {
        reason: String,
    },
    Replied {
        text: String,
    },
    /// The Supervisor judged the mission complete and shut the pilot off. Its own
    /// event (not a plain `Halted`) so the UI can mark the thread done — a purple
    /// dot + a "work complete" chime — distinct from a budget/loop/denylist stop.
    Done {
        summary: String,
    },
    Halted {
        reason: String,
    },
    /// The pilot hit a human-only step and handed control back. Carries the
    /// supervisor's explanation so the UI can show it in an alert balloon.
    Escalated {
        why: String,
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
            session_id,
            term_id: term.id,
            cwd: term.cwd,
            mission,
            pilot: Mutex::new(pilot),
            wake: Notify::new(),
        });
        self.engaged
            .lock()
            .await
            .insert(session_id, engaged.clone());

        // The watchdog drives everything from here: it sees claude sitting idle
        // (the common case — a mission set on a quiet session), debounces, and
        // sends the opening instruction itself. No separate kickoff needed.
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            this.watchdog(engaged).await;
        });
        Ok(())
    }

    pub async fn disengage(&self, session_id: AggregateId) {
        if let Some(session) = self.engaged.lock().await.remove(&session_id) {
            // Break the watchdog out of its wait so it notices it's gone and exits.
            session.wake.notify_one();
        }
    }

    fn is_engaged(&self, session_id: AggregateId, term_id: &str) -> bool {
        self.engaged
            .try_lock()
            .ok()
            .and_then(|m| m.get(&session_id).map(|s| s.term_id == term_id))
            .unwrap_or(false)
    }

    /// Consume the PTY reader's notice channel forever. A marker signal for an
    /// engaged session is only a *wake nudge* — the watchdog reads ground-truth
    /// turn state itself, so we just poke it to re-evaluate without delay.
    pub async fn run(self: Arc<Self>, mut rx: UnboundedReceiver<PureSignalNotice>) {
        while let Some(notice) = rx.recv().await {
            // Working is a keep-alive only — no decision to make.
            if matches!(notice.signal, PureSignal::Working) {
                continue;
            }
            let map = self.engaged.lock().await;
            if let Some(session) = map.get(&notice.session_id)
                && notice.terminal_id == session.term_id
            {
                session.wake.notify_one();
            }
        }
    }

    /// Per-session state-driven loop. Polls ground-truth turn state + a scrollback
    /// fingerprint; acts once per distinct settled screen, then waits for claude
    /// to start before observing again (re-sending a swallowed Enter if needed).
    async fn watchdog(self: Arc<Self>, session: Arc<EngagedSession>) {
        let session_id = session.session_id;
        let mut phase = Phase::Observe;
        // Stability tracking: the screen must read the same fingerprint for
        // RESPOND_DEBOUNCE_MS before we treat it as truly settled.
        let mut stable_fp: Option<u64> = None;
        let mut stable_since = Instant::now();
        // Dedup: the last screen we responded to. Guards against acting twice on
        // the same idle frame (the "sent the same answer twice" bug).
        let mut last_acted_fp: Option<u64> = None;

        loop {
            if !self.is_engaged(session_id, &session.term_id) {
                return;
            }
            tokio::select! {
                _ = session.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(POLL_MS)) => {}
            }
            if !self.is_engaged(session_id, &session.term_id) {
                return;
            }

            let state = self.pty.pure_state(session_id);
            let tail = self
                .pty
                .scrollback_tail(&session.term_id, CONTEXT_CHARS)
                .unwrap_or_default();
            let fp = fingerprint(&tail);

            match &mut phase {
                Phase::Observe => {
                    if state.busy {
                        // A turn is running — nothing to settle yet.
                        stable_fp = None;
                        continue;
                    }
                    // Idle: either a menu is waiting (needs_input) or the turn
                    // settled. Require the screen to hold still before acting.
                    if stable_fp != Some(fp) {
                        stable_fp = Some(fp);
                        stable_since = Instant::now();
                        continue;
                    }
                    if stable_since.elapsed() < Duration::from_millis(RESPOND_DEBOUNCE_MS) {
                        continue;
                    }
                    if last_acted_fp == Some(fp) {
                        // Already responded to this exact screen — don't double-fire.
                        continue;
                    }
                    let ask = if state.needs_input {
                        PendingKind::Permission {
                            request_id: None,
                            tool_name: None,
                            command: None,
                            raw_prompt: tail.clone(),
                        }
                    } else {
                        PendingKind::TurnEnded {
                            last_output: tail.clone(),
                        }
                    };
                    last_acted_fp = Some(fp);
                    if self.drive(session_id, &session, ask).await {
                        phase = Phase::AwaitingStart {
                            since: Instant::now(),
                            retries: 0,
                        };
                    }
                    // A non-submitting action (Halt/Escalate/error) disengaged the
                    // session; the next loop's is_engaged check returns.
                }
                Phase::AwaitingStart { since, retries } => {
                    if state.busy {
                        // claude accepted the input and started the turn.
                        phase = Phase::Observe;
                        stable_fp = None;
                        continue;
                    }
                    // Key on whether the SCREEN changed, not on needs_input. If the
                    // screen we acted on is gone — claude progressed, or a *new*
                    // prompt/menu surfaced — re-observe and decide it fresh.
                    if Some(fp) != last_acted_fp {
                        phase = Phase::Observe;
                        stable_fp = None;
                        continue;
                    }
                    // Same screen we just acted on, still idle → our Enter/submit
                    // didn't take. This covers BOTH a swallowed reply Enter AND a
                    // swallowed approve Enter on a still-open permission menu (the
                    // latter previously fell through to Observe and got blocked by
                    // the per-screen dedup → the pilot stalled on the menu).
                    if since.elapsed() < Duration::from_millis(START_TIMEOUT_MS) {
                        continue;
                    }
                    if *retries >= MAX_SUBMIT_RETRIES {
                        self.emit(
                            session_id,
                            AutopilotEvent::Error {
                                message: "claude did not accept the submitted input after \
                                          several attempts — handing back to you"
                                    .into(),
                            },
                        );
                        self.disengage(session_id).await;
                        return;
                    }
                    // Re-send a bare Enter (idempotent: an empty input box is a
                    // no-op, a filled one submits, a menu confirms the highlighted
                    // option) instead of re-asking the Supervisor.
                    let _ = self.pty.write(&session.term_id, "\r");
                    *retries += 1;
                    *since = Instant::now();
                }
            }
        }
    }

    /// Run one Supervisor step for `ask` and carry out its action. Returns `true`
    /// when the action submitted input to claude (so the caller should wait for
    /// the turn to start), `false` when it halted / errored (session disengaged).
    async fn drive(
        &self,
        session_id: AggregateId,
        session: &EngagedSession,
        ask: PendingKind,
    ) -> bool {
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

        let outcome = {
            let mut pilot = session.pilot.lock().await;
            pilot.step(&ctx, &ask).await
        };

        match outcome {
            Ok((action, reasoning)) => {
                // Surface the Supervisor's rationale (the pilot's "thinking") so
                // the UI tooltip can show *why* before the action lands.
                if let Some(text) = reasoning.filter(|r| !r.trim().is_empty()) {
                    self.emit(session_id, AutopilotEvent::Reasoning { text });
                }
                self.act(session_id, session, action).await
            }
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
                false
            }
        }
    }

    /// Carry out an [`Action`]. Returns `true` when it submitted input to claude.
    async fn act(&self, session_id: AggregateId, session: &EngagedSession, action: Action) -> bool {
        let term = &session.term_id;
        match action {
            Action::Approve => {
                // Accept the highlighted default (option 1 = Yes) with Enter.
                let _ = self.pty.write(term, "\r");
                self.emit(session_id, AutopilotEvent::Approved);
                true
            }
            Action::ApproveAlways => {
                // Prefer a "don't ask again" option so the same kind of action
                // stops prompting. The menu highlights option 1 by default, so
                // step down to the matching item with arrow-downs, then Enter.
                // No such option on screen → a plain approve (Enter).
                let tail = self
                    .pty
                    .scrollback_tail(term, CONTEXT_CHARS)
                    .unwrap_or_default();
                let downs = dont_ask_again_steps(&tail);
                for _ in 0..downs {
                    let _ = self.pty.write(term, "\x1b[B");
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
                let _ = self.pty.write(term, "\r");
                self.emit(session_id, AutopilotEvent::Approved);
                true
            }
            Action::Reject(reason) => {
                // Esc cancels the menu; then send the reason as a message.
                let _ = self.pty.write(term, "\x1b");
                tokio::time::sleep(Duration::from_millis(80)).await;
                self.submit(term, &reason).await;
                self.emit(session_id, AutopilotEvent::Rejected { reason });
                true
            }
            Action::Reply(text) => {
                self.submit(term, &text).await;
                self.emit(session_id, AutopilotEvent::Replied { text });
                true
            }
            Action::Halt(reason) => {
                self.disengage(session_id).await;
                // An escalation is the pilot saying "I can't do this — a human is
                // needed" (create an account, log in, pay, solve a CAPTCHA…). It
                // gets its own event so the UI can alert louder than a plain halt
                // (distinct chime + a balloon with the explanation).
                let event = match &reason {
                    HaltReason::Done(summary) => AutopilotEvent::Done {
                        summary: summary.clone(),
                    },
                    HaltReason::Escalated(why) => AutopilotEvent::Escalated { why: why.clone() },
                    _ => AutopilotEvent::Halted {
                        reason: halt_reason_str(&reason),
                    },
                };
                self.emit(session_id, event);
                false
            }
        }
    }

    /// Send text then a separate carriage return — claude's TUI has paste-burst
    /// detection, so `text\r` in one write (or too close together) becomes a
    /// literal newline instead of submitting. The watchdog's await-start phase
    /// re-sends the Enter if this one is still swallowed. Mirrors `sendToPty`.
    async fn submit(&self, term_id: &str, text: &str) {
        let _ = self.pty.write(term_id, text);
        tokio::time::sleep(Duration::from_millis(SUBMIT_GAP_MS)).await;
        let _ = self.pty.write(term_id, "\r");
    }

    fn emit(&self, session_id: AggregateId, event: AutopilotEvent) {
        let _ = self
            .app
            .emit(&format!("session:{session_id}:autopilot"), event);
    }
}

/// How many arrow-downs from the highlighted option 1 reach the menu item that
/// says "don't ask again", for an [`Action::ApproveAlways`]. `0` when the current
/// menu has no such option (so the caller just confirms option 1). Scoped to the
/// text after the last "do you want to …" prompt so unrelated numbered lines in
/// the scrollback (a document, a list) can't be mistaken for menu options.
fn dont_ask_again_steps(tail: &str) -> usize {
    let lower = tail.to_lowercase();
    let start = lower.rfind("do you want to").unwrap_or(0);
    for line in tail[start..].lines() {
        // Strip the selection caret / indent so "❯ 2." and "  2." both parse.
        let l = line.trim_start_matches([' ', '\t', '>', '❯', '›', '*', '-']);
        let Some((num, rest)) = l.split_once('.') else {
            continue;
        };
        let Ok(num) = num.trim().parse::<usize>() else {
            continue;
        };
        // Normalize apostrophes (don't / don’t) before matching.
        let norm = rest.to_lowercase().replace(['\'', '\u{2019}'], "");
        if norm.contains("dont ask again") {
            return num.saturating_sub(1);
        }
    }
    0
}

/// Hash of a scrollback tail with whitespace collapsed, so cosmetic TUI redraws
/// (cursor blinks, the animated tip hint already stripped upstream) read as the
/// same screen. Keys the watchdog's stability tracking and per-screen dedup.
fn fingerprint(tail: &str) -> u64 {
    let normalized = tail.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut h = DefaultHasher::new();
    normalized.hash(&mut h);
    h.finish()
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
const SYSTEM_PROMPT: &str = "You are an autonomous supervisor driving a Claude Code coding session toward a stated mission, acting in place of the user.\n\nRespond with ONLY a single JSON object, no prose, no markdown fences. Always include a \"reasoning\" field: ONE short sentence (max ~20 words, plain language) explaining why you chose this — it is shown to the user as your live thinking. The object is the \"reasoning\" field plus exactly one decision shape:\n{\"reasoning\":\"<one sentence>\",\"decision\":\"approve\"}\n{\"reasoning\":\"<one sentence>\",\"decision\":\"approve_always\"}\n{\"reasoning\":\"<one sentence>\",\"decision\":\"reject\",\"reason\":\"<why>\"}\n{\"reasoning\":\"<one sentence>\",\"decision\":\"reply\",\"text\":\"<message to send>\"}\n{\"reasoning\":\"<one sentence>\",\"decision\":\"done\",\"summary\":\"<what was accomplished>\"}\n{\"reasoning\":\"<one sentence>\",\"decision\":\"escalate\",\"why\":\"<why a human is needed>\"}\n\nGuidance: approve tool uses that safely advance the mission; reject unsafe or off-mission ones with a reason. When Claude asks a question, reply with the answer that best serves the mission. When Claude has finished a turn, decide whether the mission is complete (done) or send the next concrete instruction (reply).\n\nApprovals: use \"approve\" for a one-time yes. Use \"approve_always\" when the permission prompt offers a \"don't ask again\" option AND the mission wants this kind of action allowed standing (e.g. the user said to free up / allow whatever permissions are requested) — it picks the \"don't ask again\" option so the same action stops prompting. If no such option is shown, approve_always behaves like a plain approve, so it is safe to prefer it whenever the mission calls for granting permissions freely.\n\nEscalate (do NOT guess or reply) the moment the step needs a real human and cannot be done by typing into the coding session — for example: creating or signing into an account, entering credentials / API keys / secrets / payment details, solving a CAPTCHA, completing 2FA or email/SMS verification, granting OAuth, or any irreversible real-world action outside the repo. Put a short, specific explanation of what the human must do in \"why\". Also escalate when you are genuinely unsure or the situation looks risky.";

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
fn parse_verdict(content: &str) -> Result<Verdict, SupervisorError> {
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
    serde_json::from_str::<Verdict>(json)
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
    ) -> Result<Verdict, SupervisorError> {
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
        parse_verdict(content)
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
    ) -> Result<Verdict, SupervisorError> {
        let prompt = build_user_prompt(ctx, ask);
        let model = self.model.clone();
        let output =
            tokio::task::spawn_blocking(move || run_claude_judge(&prompt, model.as_deref()))
                .await
                .map_err(|e| SupervisorError::Backend(format!("join: {e}")))??;
        parse_verdict(&output)
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
    use oxyris_supervisor::Decision;

    #[test]
    fn parse_verdict_handles_fenced_json() {
        let v = parse_verdict("```json\n{\"decision\":\"approve\"}\n```").unwrap();
        assert!(matches!(v.decision, Decision::Approve));
        assert!(v.reasoning.is_none());
    }

    #[test]
    fn parse_verdict_handles_prose_wrapped() {
        let v = parse_verdict("Sure. {\"decision\":\"reply\",\"text\":\"go on\"} done").unwrap();
        assert!(matches!(v.decision, Decision::Reply { text } if text == "go on"));
    }

    #[test]
    fn parse_verdict_captures_reasoning() {
        let v = parse_verdict(
            "{\"reasoning\":\"Jupiter is the largest planet\",\"decision\":\"reply\",\"text\":\"C\"}",
        )
        .unwrap();
        assert_eq!(
            v.reasoning.as_deref(),
            Some("Jupiter is the largest planet")
        );
        assert!(matches!(v.decision, Decision::Reply { text } if text == "C"));
    }

    #[test]
    fn parse_verdict_rejects_garbage() {
        assert!(parse_verdict("no json here").is_err());
    }

    #[test]
    fn parse_verdict_handles_approve_always() {
        let v = parse_verdict("{\"decision\":\"approve_always\"}").unwrap();
        assert!(matches!(v.decision, Decision::ApproveAlways));
    }

    #[test]
    fn dont_ask_again_finds_option_two() {
        let menu = "Do you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again for basemaster — run_query commands in /home/x\n  3. No\n";
        // Option 2 → one arrow-down from the highlighted option 1.
        assert_eq!(dont_ask_again_steps(menu), 1);
    }

    #[test]
    fn dont_ask_again_handles_curly_apostrophe() {
        let menu =
            "Do you want to proceed?\n  1. Yes\n  2. Yes, and don\u{2019}t ask again\n  3. No";
        assert_eq!(dont_ask_again_steps(menu), 1);
    }

    #[test]
    fn dont_ask_again_absent_returns_zero() {
        // Two-option menu (no "don't ask again") → plain approve (option 1).
        let menu = "Do you want to proceed?\n❯ 1. Yes\n  2. No, and tell Claude what to do";
        assert_eq!(dont_ask_again_steps(menu), 0);
    }

    #[test]
    fn dont_ask_again_ignores_numbered_prose_above_menu() {
        // A document with "## 1." / "2." lines must not be mistaken for the menu;
        // scoping to the last "do you want to" prompt prevents it.
        let scroll = "## 1. Intro\n## 2. don't ask again (a heading, not a menu)\nlots of text\nDo you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again for X\n  3. No";
        assert_eq!(dont_ask_again_steps(scroll), 1);
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
