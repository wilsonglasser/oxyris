//! Auto-pilot supervisor abstraction.
//!
//! A [`Supervisor`] is a second LLM that drives an Oxyris session toward a
//! user-supplied [`Mission`] (a spec / changelog) — answering the prompts and
//! tool-approvals the primary `claude` would otherwise block on, with the
//! window unfocused or minimized. Concrete adapters (a multi-model client, a
//! headless `claude -p`) implement the trait; the backend's `AutopilotController`
//! owns the loop and the [`guardrails`].
//!
//! Detection lives elsewhere: pure sessions feed off `infra::pure_signals`
//! (PTY sniffing), structured sessions off the provider event stream. This crate
//! is the decision layer + the safety rails around it.

#![forbid(unsafe_code)]

pub mod controller;
pub mod guardrails;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use controller::{Action, Autopilot, HaltReason};
pub use guardrails::{Budget, Denylist, LoopGuard, LoopVerdict};

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("not configured: {0}")]
    NotConfigured(String),
    #[error("backend: {0}")]
    Backend(String),
    #[error("invalid decision from supervisor: {0}")]
    InvalidDecision(String),
}

/// The free-text goal the user pastes into the auto-pilot panel — a spec of what
/// is being built, or a changelog of what is left to do. The Supervisor treats
/// it as the objective and steers the session toward completing it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mission {
    pub text: String,
}

impl Mission {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// A mission with no usable text can't drive anything — the controller must
    /// refuse to engage.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// What the Supervisor sees of the conversation so far. Pure sessions fill
/// `recent_output` with the ANSI-stripped PTY scrollback tail; structured
/// sessions render their turn transcript into it. Kept deliberately small — the
/// Supervisor gets the mission + the latest state, not the entire history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptView {
    pub title: Option<String>,
    pub recent_output: String,
}

/// The thing the session is currently blocked on / just did, that the Supervisor
/// must react to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingKind {
    /// claude is waiting for the user to approve a tool / answer a question.
    /// Pure: the TUI's numbered menu (`raw_prompt` is the on-screen text).
    /// Structured: a `ToolApprovalRequested` (`request_id` + `tool_name` set).
    Permission {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        tool_name: Option<String>,
        /// The concrete command/args when known (e.g. a Bash invocation) — fed
        /// to the denylist. May be absent for free-form questions.
        #[serde(default)]
        command: Option<String>,
        /// The raw prompt text as shown to the user.
        raw_prompt: String,
    },
    /// The turn settled and the session is idle — the Supervisor decides whether
    /// the mission is done or what the next instruction should be.
    TurnEnded {
        #[serde(default)]
        last_output: String,
    },
}

impl PendingKind {
    /// The text the denylist should scan — the concrete command if known, else
    /// the raw prompt. `None` for a `TurnEnded` (nothing to approve).
    pub fn approval_text(&self) -> Option<&str> {
        match self {
            PendingKind::Permission {
                command: Some(cmd), ..
            } => Some(cmd),
            PendingKind::Permission { raw_prompt, .. } => Some(raw_prompt),
            PendingKind::TurnEnded { .. } => None,
        }
    }
}

/// Everything the Supervisor needs to decide one step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotContext {
    pub mission: Mission,
    pub transcript: TranscriptView,
    pub cwd: String,
}

/// The Supervisor's verdict for a single [`PendingKind`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Approve the tool / answer "yes, proceed".
    Approve,
    /// Reject the tool. `reason` is fed back to the model.
    Reject { reason: String },
    /// Reply to an open question, or send the next instruction toward the
    /// mission.
    Reply { text: String },
    /// The mission is complete — stop the pilot and notify.
    Done { summary: String },
    /// Can't decide safely — hand control back to the human.
    Escalate { why: String },
}

/// Which concrete supervisor backend to use. Mirrors the panel's selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorKind {
    /// A user-chosen model via a multi-model client.
    #[default]
    MultiModel,
    /// A headless `claude -p` instance acting as judge.
    Claude,
}

/// Implemented by each supervisor adapter. Object-safe via `async_trait` so the
/// controller can hold a `Box<dyn Supervisor>`.
#[async_trait]
pub trait Supervisor: Send + Sync {
    fn id(&self) -> &'static str;

    /// Decide what to do about `ask`, given the mission + context. Implementations
    /// must be conservative: when unsure, return [`Decision::Escalate`] rather
    /// than guessing.
    async fn decide(
        &self,
        ctx: &AutopilotContext,
        ask: &PendingKind,
    ) -> Result<Decision, SupervisorError>;
}
