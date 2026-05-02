//! Session aggregate — one conversation with a provider, owning its turns
//! and the text/blocks they accumulate. Turns are inline in the session
//! state; we'll promote Turn to its own aggregate if reflow cost bites.

use chrono::{DateTime, Utc};
use oxyris_core::{Aggregate, AggregateId, DomainEvent};
use oxyris_provider::{AssistantBlock, RuntimeMode, ThinkingMode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub struct Session;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    pub inner: Option<SessionData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionData {
    pub id: AggregateId,
    pub project_id: AggregateId,
    pub worktree_id: Option<AggregateId>,
    pub provider_id: String,
    pub model: String,
    pub thinking: ThinkingMode,
    pub runtime: RuntimeMode,
    pub status: SessionStatus,
    pub turns: Vec<TurnEntry>,
    pub created_at: DateTime<Utc>,
    /// Provider-side session id (e.g. Claude's session UUID). Populated when
    /// the provider emits SessionReady; used to resume the conversation
    /// later via `claude --resume <id>`.
    #[serde(default)]
    pub provider_session_id: Option<String>,
    /// User-facing title. Auto-generated from the first user message when
    /// missing; can be overridden via `SessionCommand::Rename`.
    #[serde(default)]
    pub title: Option<String>,
    /// Which env this session lives in — `Default` shares the project's main
    /// stack, `Worktree` uses the per-worktree isolated docker env from
    /// `.oxyris/compose.yml`. New sessions default to `Default`; the frontend
    /// auto-picks `Worktree` when the template is detected.
    #[serde(default)]
    pub env_mode: EnvMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvMode {
    #[default]
    Default,
    Worktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
    Stopped,
    Errored,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnEntry {
    pub id: String,
    pub user_text: String,
    pub blocks: Vec<AssistantBlock>,
    pub status: TurnStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub total_cost_usd: Option<f64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Streaming,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone)]
pub enum SessionCommand {
    Start {
        id: AggregateId,
        project_id: AggregateId,
        worktree_id: Option<AggregateId>,
        provider_id: String,
        model: String,
        thinking: ThinkingMode,
        runtime: RuntimeMode,
        env_mode: EnvMode,
        now: DateTime<Utc>,
    },
    SetEnvMode {
        mode: EnvMode,
    },
    Stop {
        now: DateTime<Utc>,
    },
    StartTurn {
        turn_id: String,
        user_text: String,
        now: DateTime<Utc>,
    },
    AppendAssistantBlock {
        turn_id: String,
        block: AssistantBlock,
    },
    CompleteTurn {
        turn_id: String,
        total_cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        now: DateTime<Utc>,
    },
    FailTurn {
        turn_id: String,
        message: String,
        now: DateTime<Utc>,
    },
    InterruptTurn {
        turn_id: String,
        now: DateTime<Utc>,
    },
    /// Persist the provider-assigned session id (Claude's UUID) once the
    /// provider acknowledges. Idempotent — second call with the same id is
    /// a no-op.
    AttachProviderSession {
        provider_session_id: String,
    },
    /// Move a stopped/errored session back to Running. The supervisor pairs
    /// this with respawning the provider via `--resume`.
    Resume {
        now: DateTime<Utc>,
    },
    /// Set (or change) the user-facing title of the session.
    Rename {
        title: String,
    },
    /// Mark the session as deleted. Domain stays around for replay and
    /// auditing; projections drop the row so the UI hides it.
    Delete {
        now: DateTime<Utc>,
    },
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum SessionEvent {
    SessionStarted {
        id: AggregateId,
        project_id: AggregateId,
        worktree_id: Option<AggregateId>,
        provider_id: String,
        model: String,
        thinking: ThinkingMode,
        runtime: RuntimeMode,
        #[serde(default)]
        env_mode: EnvMode,
        created_at: DateTime<Utc>,
    },
    SessionEnvModeChanged {
        mode: EnvMode,
    },
    SessionStopped {
        at: DateTime<Utc>,
    },
    SessionErrored {
        at: DateTime<Utc>,
        message: String,
    },
    TurnStarted {
        turn_id: String,
        user_text: String,
        started_at: DateTime<Utc>,
    },
    TurnAssistantBlockAppended {
        turn_id: String,
        block: AssistantBlock,
    },
    TurnCompleted {
        turn_id: String,
        total_cost_usd: Option<f64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        completed_at: DateTime<Utc>,
    },
    TurnFailed {
        turn_id: String,
        message: String,
        completed_at: DateTime<Utc>,
    },
    TurnInterrupted {
        turn_id: String,
        at: DateTime<Utc>,
    },
    ProviderSessionAttached {
        provider_session_id: String,
    },
    SessionResumed {
        at: DateTime<Utc>,
    },
    SessionRenamed {
        title: String,
    },
    SessionDeleted {
        at: DateTime<Utc>,
    },
}

impl DomainEvent for SessionEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "SessionStarted",
            Self::SessionStopped { .. } => "SessionStopped",
            Self::SessionErrored { .. } => "SessionErrored",
            Self::TurnStarted { .. } => "TurnStarted",
            Self::TurnAssistantBlockAppended { .. } => "TurnAssistantBlockAppended",
            Self::TurnCompleted { .. } => "TurnCompleted",
            Self::TurnFailed { .. } => "TurnFailed",
            Self::TurnInterrupted { .. } => "TurnInterrupted",
            Self::ProviderSessionAttached { .. } => "ProviderSessionAttached",
            Self::SessionResumed { .. } => "SessionResumed",
            Self::SessionRenamed { .. } => "SessionRenamed",
            Self::SessionDeleted { .. } => "SessionDeleted",
            Self::SessionEnvModeChanged { .. } => "SessionEnvModeChanged",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("session already exists")]
    AlreadyExists,
    #[error("session does not exist")]
    NotFound,
    #[error("session is not running")]
    NotRunning,
    #[error("unknown turn {0}")]
    UnknownTurn(String),
    #[error("turn {0} is not streaming")]
    TurnNotStreaming(String),
    #[error("session must be stopped or errored to resume")]
    NotResumable,
    #[error("session has no provider session id captured yet — cannot resume")]
    NoProviderSession,
    #[error("title must not be empty")]
    EmptyTitle,
}

impl Aggregate for Session {
    const KIND: &'static str = "session";
    type Command = SessionCommand;
    type Event = SessionEvent;
    type State = SessionState;
    type Error = SessionError;

    fn decide(state: &Self::State, cmd: Self::Command) -> Result<Vec<Self::Event>, Self::Error> {
        match cmd {
            SessionCommand::Start {
                id,
                project_id,
                worktree_id,
                provider_id,
                model,
                thinking,
                runtime,
                env_mode,
                now,
            } => {
                if state.inner.is_some() {
                    return Err(SessionError::AlreadyExists);
                }
                Ok(vec![SessionEvent::SessionStarted {
                    id,
                    project_id,
                    worktree_id,
                    provider_id,
                    model,
                    thinking,
                    runtime,
                    env_mode,
                    created_at: now,
                }])
            }
            SessionCommand::SetEnvMode { mode } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                if data.env_mode == mode {
                    return Ok(vec![]);
                }
                Ok(vec![SessionEvent::SessionEnvModeChanged { mode }])
            }
            SessionCommand::Stop { now } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                if data.status == SessionStatus::Stopped {
                    return Ok(vec![]);
                }
                Ok(vec![SessionEvent::SessionStopped { at: now }])
            }
            SessionCommand::StartTurn {
                turn_id,
                user_text,
                now,
            } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                if data.status == SessionStatus::Stopped {
                    return Err(SessionError::NotRunning);
                }
                if data.turns.iter().any(|t| t.id == turn_id) {
                    return Err(SessionError::UnknownTurn(format!("duplicate {turn_id}")));
                }
                Ok(vec![SessionEvent::TurnStarted {
                    turn_id,
                    user_text,
                    started_at: now,
                }])
            }
            SessionCommand::AppendAssistantBlock { turn_id, block } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                let turn = data
                    .turns
                    .iter()
                    .find(|t| t.id == turn_id)
                    .ok_or_else(|| SessionError::UnknownTurn(turn_id.clone()))?;
                if turn.status != TurnStatus::Streaming {
                    return Err(SessionError::TurnNotStreaming(turn_id));
                }
                Ok(vec![SessionEvent::TurnAssistantBlockAppended {
                    turn_id,
                    block,
                }])
            }
            SessionCommand::CompleteTurn {
                turn_id,
                total_cost_usd,
                input_tokens,
                output_tokens,
                now,
            } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                let turn = data
                    .turns
                    .iter()
                    .find(|t| t.id == turn_id)
                    .ok_or_else(|| SessionError::UnknownTurn(turn_id.clone()))?;
                if turn.status != TurnStatus::Streaming {
                    return Ok(vec![]); // idempotent
                }
                Ok(vec![SessionEvent::TurnCompleted {
                    turn_id,
                    total_cost_usd,
                    input_tokens,
                    output_tokens,
                    completed_at: now,
                }])
            }
            SessionCommand::FailTurn {
                turn_id,
                message,
                now,
            } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                let _ = data
                    .turns
                    .iter()
                    .find(|t| t.id == turn_id)
                    .ok_or_else(|| SessionError::UnknownTurn(turn_id.clone()))?;
                Ok(vec![SessionEvent::TurnFailed {
                    turn_id,
                    message,
                    completed_at: now,
                }])
            }
            SessionCommand::InterruptTurn { turn_id, now } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                let _ = data
                    .turns
                    .iter()
                    .find(|t| t.id == turn_id)
                    .ok_or_else(|| SessionError::UnknownTurn(turn_id.clone()))?;
                Ok(vec![SessionEvent::TurnInterrupted { turn_id, at: now }])
            }
            SessionCommand::AttachProviderSession {
                provider_session_id,
            } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                if data.provider_session_id.as_deref() == Some(provider_session_id.as_str()) {
                    return Ok(vec![]);
                }
                Ok(vec![SessionEvent::ProviderSessionAttached {
                    provider_session_id,
                }])
            }
            SessionCommand::Resume { now } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                if !matches!(data.status, SessionStatus::Stopped | SessionStatus::Errored) {
                    return Err(SessionError::NotResumable);
                }
                if data.provider_session_id.is_none() {
                    return Err(SessionError::NoProviderSession);
                }
                Ok(vec![SessionEvent::SessionResumed { at: now }])
            }
            SessionCommand::Rename { title } => {
                let data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                let trimmed = title.trim().to_owned();
                if trimmed.is_empty() {
                    return Err(SessionError::EmptyTitle);
                }
                if data.title.as_deref() == Some(trimmed.as_str()) {
                    return Ok(vec![]);
                }
                Ok(vec![SessionEvent::SessionRenamed { title: trimmed }])
            }
            SessionCommand::Delete { now } => {
                let _data = state.inner.as_ref().ok_or(SessionError::NotFound)?;
                Ok(vec![SessionEvent::SessionDeleted { at: now }])
            }
        }
    }

    fn apply(state: &mut Self::State, event: &Self::Event) {
        match event {
            SessionEvent::SessionStarted {
                id,
                project_id,
                worktree_id,
                provider_id,
                model,
                thinking,
                runtime,
                env_mode,
                created_at,
            } => {
                state.inner = Some(SessionData {
                    id: *id,
                    project_id: *project_id,
                    worktree_id: *worktree_id,
                    provider_id: provider_id.clone(),
                    model: model.clone(),
                    thinking: *thinking,
                    runtime: *runtime,
                    status: SessionStatus::Running,
                    turns: Vec::new(),
                    created_at: *created_at,
                    provider_session_id: None,
                    title: None,
                    env_mode: *env_mode,
                });
            }
            SessionEvent::SessionEnvModeChanged { mode } => {
                if let Some(data) = state.inner.as_mut() {
                    data.env_mode = *mode;
                }
            }
            SessionEvent::SessionStopped { .. } => {
                if let Some(data) = state.inner.as_mut() {
                    data.status = SessionStatus::Stopped;
                }
            }
            SessionEvent::SessionErrored { .. } => {
                if let Some(data) = state.inner.as_mut() {
                    data.status = SessionStatus::Errored;
                }
            }
            SessionEvent::TurnStarted {
                turn_id,
                user_text,
                started_at,
            } => {
                if let Some(data) = state.inner.as_mut() {
                    data.turns.push(TurnEntry {
                        id: turn_id.clone(),
                        user_text: user_text.clone(),
                        blocks: Vec::new(),
                        status: TurnStatus::Streaming,
                        started_at: *started_at,
                        completed_at: None,
                        total_cost_usd: None,
                        input_tokens: None,
                        output_tokens: None,
                        error_message: None,
                    });
                }
            }
            SessionEvent::TurnAssistantBlockAppended { turn_id, block } => {
                if let Some(data) = state.inner.as_mut()
                    && let Some(turn) = data.turns.iter_mut().find(|t| &t.id == turn_id)
                {
                    merge_block(&mut turn.blocks, block.clone());
                }
            }
            SessionEvent::TurnCompleted {
                turn_id,
                total_cost_usd,
                input_tokens,
                output_tokens,
                completed_at,
            } => {
                if let Some(data) = state.inner.as_mut()
                    && let Some(turn) = data.turns.iter_mut().find(|t| &t.id == turn_id)
                {
                    turn.status = TurnStatus::Completed;
                    turn.completed_at = Some(*completed_at);
                    turn.total_cost_usd = *total_cost_usd;
                    turn.input_tokens = *input_tokens;
                    turn.output_tokens = *output_tokens;
                }
            }
            SessionEvent::TurnFailed {
                turn_id,
                message,
                completed_at,
            } => {
                if let Some(data) = state.inner.as_mut()
                    && let Some(turn) = data.turns.iter_mut().find(|t| &t.id == turn_id)
                {
                    turn.status = TurnStatus::Failed;
                    turn.completed_at = Some(*completed_at);
                    turn.error_message = Some(message.clone());
                }
            }
            SessionEvent::TurnInterrupted { turn_id, at } => {
                if let Some(data) = state.inner.as_mut()
                    && let Some(turn) = data.turns.iter_mut().find(|t| &t.id == turn_id)
                {
                    turn.status = TurnStatus::Interrupted;
                    turn.completed_at = Some(*at);
                }
            }
            SessionEvent::ProviderSessionAttached {
                provider_session_id,
            } => {
                if let Some(data) = state.inner.as_mut() {
                    data.provider_session_id = Some(provider_session_id.clone());
                }
            }
            SessionEvent::SessionResumed { .. } => {
                if let Some(data) = state.inner.as_mut() {
                    data.status = SessionStatus::Running;
                }
            }
            SessionEvent::SessionRenamed { title } => {
                if let Some(data) = state.inner.as_mut() {
                    data.title = Some(title.clone());
                }
            }
            SessionEvent::SessionDeleted { .. } => {
                state.inner = None;
            }
        }
    }
}

/// Merge consecutive text blocks so the UI sees one coalesced text bubble
/// instead of dozens of append events. Tool-use and other blocks always get
/// their own entry.
fn merge_block(blocks: &mut Vec<AssistantBlock>, incoming: AssistantBlock) {
    if let (AssistantBlock::Text { text: next }, Some(AssistantBlock::Text { text: prev })) =
        (&incoming, blocks.last_mut())
    {
        prev.push_str(next);
        return;
    }
    blocks.push(incoming);
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxyris_core::replay;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn start() -> SessionCommand {
        SessionCommand::Start {
            id: AggregateId::new(),
            project_id: AggregateId::new(),
            worktree_id: None,
            provider_id: "claude".into(),
            model: "claude-opus-4-7".into(),
            thinking: ThinkingMode::Auto,
            runtime: RuntimeMode::Supervised,
            env_mode: EnvMode::Default,
            now: now(),
        }
    }

    #[test]
    fn start_twice_rejected() {
        let mut state = SessionState::default();
        for e in Session::decide(&state, start()).unwrap() {
            Session::apply(&mut state, &e);
        }
        assert_eq!(
            Session::decide(&state, start()).unwrap_err(),
            SessionError::AlreadyExists
        );
    }

    #[test]
    fn text_blocks_coalesce() {
        let mut state = SessionState::default();
        for e in Session::decide(&state, start()).unwrap() {
            Session::apply(&mut state, &e);
        }
        for e in Session::decide(
            &state,
            SessionCommand::StartTurn {
                turn_id: "t1".into(),
                user_text: "hi".into(),
                now: now(),
            },
        )
        .unwrap()
        {
            Session::apply(&mut state, &e);
        }
        for text in ["He", "llo", ", wor", "ld"] {
            for e in Session::decide(
                &state,
                SessionCommand::AppendAssistantBlock {
                    turn_id: "t1".into(),
                    block: AssistantBlock::Text {
                        text: text.to_owned(),
                    },
                },
            )
            .unwrap()
            {
                Session::apply(&mut state, &e);
            }
        }
        let turn = &state.inner.as_ref().unwrap().turns[0];
        assert_eq!(turn.blocks.len(), 1);
        match &turn.blocks[0] {
            AssistantBlock::Text { text } => assert_eq!(text, "Hello, world"),
            other => panic!("unexpected block: {other:?}"),
        }
    }

    #[test]
    fn complete_turn_updates_status_and_replay_matches() {
        let mut state = SessionState::default();
        let mut all = Vec::new();
        for cmd in [
            start(),
            SessionCommand::StartTurn {
                turn_id: "t1".into(),
                user_text: "q".into(),
                now: now(),
            },
            SessionCommand::AppendAssistantBlock {
                turn_id: "t1".into(),
                block: AssistantBlock::Text { text: "a".into() },
            },
            SessionCommand::CompleteTurn {
                turn_id: "t1".into(),
                total_cost_usd: Some(0.001),
                input_tokens: Some(3),
                output_tokens: Some(1),
                now: now(),
            },
        ] {
            let evs = Session::decide(&state, cmd).unwrap();
            for e in &evs {
                Session::apply(&mut state, e);
            }
            all.extend(evs);
        }
        let turn = &state.inner.as_ref().unwrap().turns[0];
        assert_eq!(turn.status, TurnStatus::Completed);
        assert_eq!(turn.total_cost_usd, Some(0.001));

        let replayed = replay::<Session>(&all);
        assert_eq!(state, replayed);
    }

    #[test]
    fn stop_session_moves_status_and_rejects_new_turns() {
        let mut state = SessionState::default();
        for e in Session::decide(&state, start()).unwrap() {
            Session::apply(&mut state, &e);
        }
        for e in Session::decide(&state, SessionCommand::Stop { now: now() }).unwrap() {
            Session::apply(&mut state, &e);
        }
        assert_eq!(state.inner.as_ref().unwrap().status, SessionStatus::Stopped);
        assert_eq!(
            Session::decide(
                &state,
                SessionCommand::StartTurn {
                    turn_id: "x".into(),
                    user_text: "y".into(),
                    now: now(),
                }
            )
            .unwrap_err(),
            SessionError::NotRunning
        );
    }
}
