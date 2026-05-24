//! Generic AI provider abstraction.
//!
//! Concrete providers (Claude in `oxyris-claude`, future Codex / Cursor /
//! OpenCode) implement [`Provider`] and are assembled into a
//! [`ProviderRegistry`] at startup. The backend only ever talks to the trait
//! surface so adding a provider is a new crate, not a refactor.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use oxyris_core::Environment;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("spawn: {0}")]
    Spawn(String),
    #[error("auth required")]
    NotAuthenticated,
    #[error("unsupported model: {0}")]
    UnsupportedModel(String),
    #[error("provider crashed: {0}")]
    Crashed(String),
    #[error("other: {0}")]
    Other(String),
}

// ────── data ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOptions {
    pub environment: Environment,
    pub cwd: String,
    pub model: String,
    #[serde(default)]
    pub thinking: ThinkingMode,
    #[serde(default)]
    pub runtime: RuntimeMode,
    /// Optional system prompt override (appended to the provider's default).
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// When set, the provider should resume this prior session id rather
    /// than start a fresh one (e.g. Claude's `--resume <id>`).
    #[serde(default)]
    pub resume_session_id: Option<String>,
    /// Optional path to an MCP-server config JSON. Claude consumes this via
    /// `--mcp-config <path>` and exposes the listed servers as tool sources.
    #[serde(default)]
    pub mcp_config_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    #[default]
    Auto,
    Off,
    On,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    /// Approval required for every tool invocation. Maps to Claude's
    /// `--permission-mode default`.
    #[default]
    Supervised,
    /// File edits run without approval, but other tools still ask.
    /// Maps to `--permission-mode acceptEdits`.
    AcceptEdits,
    /// No approval prompts. Tools execute immediately. Maps to
    /// `--permission-mode bypassPermissions`.
    FullAccess,
    /// Read-only planning — no code execution, just proposals. Maps to
    /// `--permission-mode plan`.
    Plan,
}

/// A single piece of assistant output. Streaming providers emit a sequence of
/// these per turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssistantBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        output: serde_json::Value,
        is_error: bool,
    },
}

/// Events flowing from the provider back to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderEvent {
    /// Emitted when the provider acknowledges the session and is ready to
    /// accept input.
    SessionReady {
        provider_session_id: Option<String>,
        model: String,
    },
    /// One block of assistant output arrived.
    AssistantBlock {
        turn_id: String,
        block: AssistantBlock,
    },
    /// Incremental text delta for the current text block — providers that
    /// deliver character-by-character use this; block-granular providers skip it.
    AssistantTextDelta { turn_id: String, text: String },
    /// The current turn has fully finished.
    TurnCompleted {
        turn_id: String,
        total_cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    /// The current turn failed before completing.
    TurnFailed { turn_id: String, message: String },
    /// The provider is asking the user to approve a tool invocation before it
    /// runs (supervised / accept-edits). The turn is paused until the backend
    /// answers with [`ProviderCommand::ApproveToolUse`] or
    /// [`ProviderCommand::RejectToolUse`], keyed by `request_id`.
    ToolApprovalRequested {
        turn_id: String,
        /// Provider-assigned id for this approval round-trip. Echoed back in
        /// the approve/reject command — distinct from `tool_use_id`.
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    /// The session exited — the supervisor should tear it down.
    SessionEnded,
}

/// Commands flowing from the backend into the provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderCommand {
    /// Append a user message and start a turn.
    SendMessage { turn_id: String, text: String },
    /// Interrupt the in-flight turn.
    Interrupt,
    /// Approve a pending tool-use (supervised mode). `request_id` is the id
    /// from the matching [`ProviderEvent::ToolApprovalRequested`].
    ApproveToolUse { request_id: String },
    /// Reject a pending tool-use. `message` is fed back to the model as the
    /// reason the call was denied.
    RejectToolUse { request_id: String, message: String },
    /// Ask the provider to shut down cleanly.
    Stop,
}

/// Capability snapshot for a provider installation in a given environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub provider_id: String,
    pub display_name: String,
    pub version: Option<String>,
    pub authenticated: bool,
    pub models: Vec<ModelDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub supports_thinking: bool,
}

// ────── trait ─────────────────────────────────────────────────────────────

/// Handle to one live session with a provider. Callers pump commands in and
/// drain events out.
pub struct ProviderSession {
    pub commands: mpsc::UnboundedSender<ProviderCommand>,
    pub events: mpsc::UnboundedReceiver<ProviderEvent>,
    /// Optional provider-assigned ID (Claude calls these session IDs; not
    /// every provider exposes them).
    pub provider_session_id: Option<String>,
}

/// Implemented by each provider adapter. All methods are sync on the trait so
/// Rust's object safety is cheap; start_session kicks off background tasks
/// internally.
pub trait Provider: Send + Sync {
    /// Stable identifier, e.g. `"claude"`, `"codex"`.
    fn id(&self) -> &'static str;

    /// Human-facing display name for settings screens.
    fn display_name(&self) -> &'static str;

    /// Start a new session. Returns the command/event channels immediately;
    /// the session is "live" once it emits [`ProviderEvent::SessionReady`].
    fn start_session(&self, opts: SessionOptions) -> Result<ProviderSession, ProviderError>;
}

// ────── registry ──────────────────────────────────────────────────────────

/// Starts with zero providers and receives them via [`ProviderRegistry::register`].
/// The backend wires this up at boot.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<&'static str, Box<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        self.providers.insert(provider.id(), provider);
    }

    pub fn get(&self, id: &str) -> Option<&dyn Provider> {
        self.providers.get(id).map(|b| b.as_ref())
    }

    pub fn ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.providers.keys().copied()
    }
}
