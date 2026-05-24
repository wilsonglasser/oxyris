//! Session supervisor — owns one running [`ProviderSession`] per Oxyris
//! session and pumps provider events through the Session aggregate.
//!
//! For every provider event we receive, we:
//!
//! 1. translate it into a [`SessionCommand`],
//! 2. ask the aggregate's `decide` for events,
//! 3. append them under the current expected version,
//! 4. apply each event to the read model (no projection yet — Sprint 7),
//! 5. emit a typed Tauri event so the UI can re-render.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use oxyris_core::{Aggregate, AggregateId, replay};
use oxyris_provider::{
    ProviderCommand, ProviderError, ProviderEvent, ProviderRegistry, ProviderSession,
    SessionOptions,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::domain::session::{Session, SessionCommand, SessionError, SessionEvent, SessionState};
use crate::infra::agent_pool::AgentPool;
use crate::infra::checkpoint::{self, Phase};
use crate::infra::event_store::{EventStore, EventStoreError};
use crate::infra::mcp;
use crate::infra::projections::{ProjectionError, Projections};

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("unknown provider {0}")]
    UnknownProvider(String),
    #[error("provider: {0}")]
    Provider(#[from] ProviderError),
    #[error("domain: {0}")]
    Domain(#[from] SessionError),
    #[error("storage: {0}")]
    Storage(#[from] EventStoreError),
    #[error("projection: {0}")]
    Projection(#[from] ProjectionError),
    #[error("session not running: {0}")]
    SessionNotRunning(AggregateId),
}

/// What we ship to the frontend on each persisted SessionEvent.
#[derive(Debug, Clone, Serialize)]
pub struct EmittedSessionEvent {
    pub session_id: AggregateId,
    pub version: u32,
    pub event: SessionEvent,
}

struct LiveSession {
    commands: tokio::sync::mpsc::UnboundedSender<ProviderCommand>,
}

pub struct SessionSupervisor {
    registry: Arc<ProviderRegistry>,
    event_store: Arc<EventStore>,
    projections: Arc<Projections>,
    agent_pool: Arc<AgentPool>,
    app: AppHandle,
    live: Mutex<HashMap<AggregateId, LiveSession>>,
    /// Shared with `AppState`. Read at session-spawn time so the
    /// per-worktree `mcp.json` includes `--lsp-bridge` when the bridge is
    /// up. `None` until the bridge has bound.
    lsp_bridge_port: Arc<std::sync::Mutex<Option<u16>>>,
}

impl SessionSupervisor {
    pub fn new(
        registry: Arc<ProviderRegistry>,
        event_store: Arc<EventStore>,
        projections: Arc<Projections>,
        agent_pool: Arc<AgentPool>,
        app: AppHandle,
        lsp_bridge_port: Arc<std::sync::Mutex<Option<u16>>>,
    ) -> Self {
        Self {
            registry,
            event_store,
            projections,
            agent_pool,
            app,
            live: Mutex::new(HashMap::new()),
            lsp_bridge_port,
        }
    }

    pub fn lsp_bridge_port(&self) -> Option<u16> {
        self.lsp_bridge_port.lock().ok().and_then(|guard| *guard)
    }

    /// Start a new session: persist `SessionStarted`, spawn the provider, and
    /// wire its event stream into the aggregate.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_session(
        &self,
        project_id: AggregateId,
        worktree_id: Option<AggregateId>,
        provider_id: String,
        env_mode: crate::domain::session::EnvMode,
        kind: crate::domain::session::SessionKind,
        opts: SessionOptions,
    ) -> Result<AggregateId, SupervisorError> {
        use crate::domain::session::SessionKind;

        // Structured sessions need a registered provider up front; Pure
        // sessions run the interactive TUI in a PTY (spawned later by the UI)
        // so they don't touch the stream-json provider registry at all.
        let provider = match kind {
            SessionKind::Structured => Some(
                self.registry
                    .get(&provider_id)
                    .ok_or_else(|| SupervisorError::UnknownProvider(provider_id.clone()))?,
            ),
            SessionKind::Pure => None,
        };

        let session_id = AggregateId::new();
        let now = Utc::now();
        let cmd = SessionCommand::Start {
            id: session_id,
            project_id,
            worktree_id,
            provider_id: provider_id.clone(),
            model: opts.model.clone(),
            thinking: opts.thinking,
            runtime: opts.runtime,
            env_mode,
            kind,
            now,
        };
        let events = Session::decide(&SessionState::default(), cmd)?;
        self.persist_and_emit(session_id, 0, &events).await?;

        // Pure mode stops here: the aggregate exists (so the session shows in
        // the sidebar with its cwd/title), but the conversation happens in the
        // PTY, not over stream-json. No provider, no event pump.
        if let Some(provider) = provider {
            let opts = augment_with_mcp(opts, self.lsp_bridge_port());
            let provider_session = provider.start_session(opts)?;
            self.spawn_event_pump(session_id, provider_session).await;
        }

        Ok(session_id)
    }

    /// Append a user message and forward it to the provider.
    pub async fn send_user_message(
        &self,
        session_id: AggregateId,
        text: String,
    ) -> Result<String, SupervisorError> {
        let (state, version) = self.load_session(session_id)?;
        let turn_id = format!("turn-{}", uuid::Uuid::now_v7());
        let now = Utc::now();

        // Auto-title the session from its first user message when it has no
        // title yet. Tighter threshold than just "non-empty": short prompts
        // like "hi" or "test" produce noise titles, so we skip anything under
        // ~15 useful chars. The user can always rename manually.
        let mut events = Session::decide(
            &state,
            SessionCommand::StartTurn {
                turn_id: turn_id.clone(),
                user_text: text.clone(),
                now,
            },
        )?;
        let needs_auto_title = state
            .inner
            .as_ref()
            .is_some_and(|d| d.title.is_none() && d.turns.is_empty());
        if needs_auto_title && let Some(title) = derive_auto_title(&text) {
            events.push(SessionEvent::SessionRenamed { title });
        }

        self.persist_and_emit(session_id, version, &events).await?;

        // Pre-turn checkpoint — best-effort. Failures are logged but don't
        // block the turn, since the diff UI is a nice-to-have, not correctness.
        self.capture_checkpoint(session_id, &turn_id, Phase::Pre)
            .await;

        let live = self.live.lock().await;
        let live = live
            .get(&session_id)
            .ok_or(SupervisorError::SessionNotRunning(session_id))?;
        let _ = live.commands.send(ProviderCommand::SendMessage {
            turn_id: turn_id.clone(),
            text,
        });
        Ok(turn_id)
    }

    async fn capture_checkpoint(&self, session_id: AggregateId, turn_id: &str, phase: Phase) {
        let Some((root_path, environment)) = self.resolve_project_for_session(session_id) else {
            return;
        };
        let turn_id_owned = turn_id.to_owned();
        let session_str = session_id.to_string();
        let result = checkpoint::capture(
            &environment,
            &self.agent_pool,
            &root_path,
            &session_str,
            &turn_id_owned,
            phase,
        )
        .await;
        match result {
            Ok(None) => {}
            Ok(Some(ref_name)) => {
                tracing::debug!(%session_id, ref_name, ?phase, "captured checkpoint");
            }
            Err(e) => {
                tracing::warn!(error = %e, %session_id, ?phase, "checkpoint capture failed");
            }
        }
    }

    fn resolve_project_for_session(
        &self,
        session_id: AggregateId,
    ) -> Option<(String, oxyris_core::Environment)> {
        let snap = self.projections.get_session(session_id).ok().flatten()?;
        let project_id = snap.data.project_id;
        let projects = self.projections.list_projects().ok()?;
        let p = projects.into_iter().find(|p| p.id == project_id)?;
        Some((p.root_path, p.environment))
    }

    pub async fn rename_session(
        &self,
        session_id: AggregateId,
        title: String,
    ) -> Result<(), SupervisorError> {
        let (state, version) = self.load_session(session_id)?;
        let events = Session::decide(&state, SessionCommand::Rename { title })?;
        self.persist_and_emit(session_id, version, &events).await
    }

    /// Pin or unpin a session. Idempotent at the aggregate level; returns
    /// without doing anything if the state hasn't changed.
    pub async fn toggle_pin(&self, session_id: AggregateId) -> Result<(), SupervisorError> {
        let (state, version) = self.load_session(session_id)?;
        let events = Session::decide(&state, SessionCommand::TogglePin { now: Utc::now() })?;
        self.persist_and_emit(session_id, version, &events).await
    }

    pub async fn set_env_mode(
        &self,
        session_id: AggregateId,
        mode: crate::domain::session::EnvMode,
    ) -> Result<(), SupervisorError> {
        let (state, version) = self.load_session(session_id)?;
        let events = Session::decide(&state, SessionCommand::SetEnvMode { mode })?;
        self.persist_and_emit(session_id, version, &events).await
    }

    pub async fn delete_session(&self, session_id: AggregateId) -> Result<(), SupervisorError> {
        // Stop it if it's still running so no dangling PTY / provider process
        // lives past the delete.
        {
            let mut live = self.live.lock().await;
            if let Some(live) = live.remove(&session_id) {
                let _ = live.commands.send(ProviderCommand::Stop);
            }
        }
        let (state, version) = self.load_session(session_id)?;
        let events = Session::decide(&state, SessionCommand::Delete { now: Utc::now() })?;
        self.persist_and_emit(session_id, version, &events).await
    }

    pub async fn interrupt(
        &self,
        session_id: AggregateId,
        turn_id: String,
    ) -> Result<(), SupervisorError> {
        let (state, version) = self.load_session(session_id)?;
        let now = Utc::now();
        let events = Session::decide(&state, SessionCommand::InterruptTurn { turn_id, now })?;
        self.persist_and_emit(session_id, version, &events).await?;
        let live = self.live.lock().await;
        if let Some(live) = live.get(&session_id) {
            let _ = live.commands.send(ProviderCommand::Interrupt);
        }
        Ok(())
    }

    /// Answer a pending tool-approval prompt. `request_id` comes from the
    /// `session:{id}:approval` event. No event is persisted — the decision
    /// only unblocks the provider's in-flight turn.
    pub async fn approve_tool_use(
        &self,
        session_id: AggregateId,
        request_id: String,
    ) -> Result<(), SupervisorError> {
        let live = self.live.lock().await;
        if let Some(live) = live.get(&session_id) {
            let _ = live
                .commands
                .send(ProviderCommand::ApproveToolUse { request_id });
        }
        Ok(())
    }

    pub async fn reject_tool_use(
        &self,
        session_id: AggregateId,
        request_id: String,
        message: String,
    ) -> Result<(), SupervisorError> {
        let live = self.live.lock().await;
        if let Some(live) = live.get(&session_id) {
            let _ = live.commands.send(ProviderCommand::RejectToolUse {
                request_id,
                message,
            });
        }
        Ok(())
    }

    pub async fn stop_session(&self, session_id: AggregateId) -> Result<(), SupervisorError> {
        let (state, version) = self.load_session(session_id)?;
        let events = Session::decide(&state, SessionCommand::Stop { now: Utc::now() })?;
        self.persist_and_emit(session_id, version, &events).await?;

        let mut live = self.live.lock().await;
        if let Some(live) = live.remove(&session_id) {
            let _ = live.commands.send(ProviderCommand::Stop);
        }
        Ok(())
    }

    /// Bring a stopped/errored session back to life. Spawns the provider with
    /// `--resume <provider_session_id>` so Claude restores the prior thread,
    /// then wires the event pump exactly like a fresh start.
    pub async fn resume_session(&self, session_id: AggregateId) -> Result<(), SupervisorError> {
        let (state, version) = self.load_session(session_id)?;
        let data = state
            .inner
            .as_ref()
            .ok_or(SupervisorError::Domain(SessionError::NotFound))?
            .clone();

        // Pure sessions have no stream-json provider to resume — the PTY is
        // owned by the UI and respawned on demand. Nothing to do here.
        if data.kind == crate::domain::session::SessionKind::Pure {
            return Ok(());
        }

        let resume_id = data
            .provider_session_id
            .clone()
            .ok_or(SupervisorError::Domain(SessionError::NoProviderSession))?;

        // Persist the SessionResumed event before we touch the provider so a
        // crash mid-spawn still leaves the aggregate consistent.
        let events = Session::decide(&state, SessionCommand::Resume { now: Utc::now() })?;
        self.persist_and_emit(session_id, version, &events).await?;

        // Resolve project context for cwd + environment.
        let (root_path, environment) = self
            .resolve_project_for_session(session_id)
            .ok_or(SupervisorError::Domain(SessionError::NotFound))?;

        let provider = self
            .registry
            .get(&data.provider_id)
            .ok_or_else(|| SupervisorError::UnknownProvider(data.provider_id.clone()))?;

        let opts = SessionOptions {
            environment,
            cwd: root_path,
            model: data.model.clone(),
            thinking: data.thinking,
            runtime: data.runtime,
            system_prompt: None,
            resume_session_id: Some(resume_id),
            mcp_config_path: None,
        };
        let opts = augment_with_mcp(opts, self.lsp_bridge_port());
        let provider_session = provider.start_session(opts)?;
        self.spawn_event_pump(session_id, provider_session).await;
        Ok(())
    }

    fn load_session(
        &self,
        session_id: AggregateId,
    ) -> Result<(SessionState, u32), SupervisorError> {
        let stored = self.event_store.load(Session::KIND, session_id)?;
        let mut typed = Vec::with_capacity(stored.len());
        for s in &stored {
            let event: SessionEvent = serde_json::from_value(s.payload.clone())
                .map_err(EventStoreError::Serialization)?;
            typed.push(event);
        }
        let version = stored.last().map(|s| s.version).unwrap_or(0);
        Ok((replay::<Session>(&typed), version))
    }

    async fn persist_and_emit(
        &self,
        session_id: AggregateId,
        expected_version: u32,
        events: &[SessionEvent],
    ) -> Result<(), SupervisorError> {
        if events.is_empty() {
            return Ok(());
        }
        let stored =
            self.event_store
                .append(Session::KIND, session_id, expected_version, events)?;
        for s in &stored {
            // Keep the read model in lockstep — `session_get` hits the
            // projection, not the event log, so a missed apply here makes
            // the UI think nothing happened.
            self.projections.apply(s)?;
        }
        for (s, ev) in stored.iter().zip(events.iter()) {
            let payload = EmittedSessionEvent {
                session_id,
                version: s.version,
                event: ev.clone(),
            };
            let event_name = format!("session:{session_id}:event");
            let _ = self.app.emit(&event_name, payload);
        }
        Ok(())
    }

    async fn spawn_event_pump(&self, session_id: AggregateId, mut session: ProviderSession) {
        let store = self.event_store.clone();
        let projections = self.projections.clone();
        let agent_pool = self.agent_pool.clone();
        let app = self.app.clone();
        {
            let mut live = self.live.lock().await;
            live.insert(
                session_id,
                LiveSession {
                    commands: session.commands.clone(),
                },
            );
        }

        tokio::spawn(async move {
            while let Some(event) = session.events.recv().await {
                if let Err(e) = handle_provider_event(
                    &store,
                    &projections,
                    &agent_pool,
                    &app,
                    session_id,
                    event,
                )
                .await
                {
                    tracing::warn!(error = %e, "provider event handling failed");
                }
            }
            tracing::debug!(session = %session_id, "provider event stream ended");
        });
    }
}

/// Build an auto-title from a user message. Returns `None` when the input
/// is too short to be a useful title — caller should leave the title
/// untouched in that case so the next message gets another shot. Uses the
/// first non-empty line, trimmed, capped at 60 chars.
fn derive_auto_title(text: &str) -> Option<String> {
    const MIN_USEFUL_CHARS: usize = 15;
    let first = text.trim().lines().find(|l| !l.trim().is_empty())?.trim();
    if first.chars().count() < MIN_USEFUL_CHARS {
        return None;
    }
    let mut chars = first.chars().take(60).collect::<String>();
    // If we cut mid-word, trim trailing whitespace that the truncate left.
    let trimmed_end = chars.trim_end().to_owned();
    chars.clear();
    chars.push_str(&trimmed_end);
    Some(chars)
}

/// Generate the per-worktree MCP config (best-effort) and stitch it into
/// `opts`: sets `mcp_config_path` and appends the system-prompt nudge so
/// Claude knows the tools exist. WSL projects and missing-binary cases skip
/// silently — the provider runs without MCP, which is the same as before.
fn augment_with_mcp(mut opts: SessionOptions, lsp_bridge_port: Option<u16>) -> SessionOptions {
    let setup = match mcp::prepare_for_worktree(&opts.environment, &opts.cwd, lsp_bridge_port) {
        Ok(Some(s)) => s,
        Ok(None) => return opts,
        Err(e) => {
            tracing::warn!(error = %e, "mcp config write failed; provider will run without it");
            return opts;
        }
    };
    opts.mcp_config_path = Some(setup.config_path);
    opts.system_prompt = Some(match opts.system_prompt {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{existing}\n\n{}", setup.system_prompt_nudge)
        }
        _ => setup.system_prompt_nudge,
    });
    opts
}

async fn handle_provider_event(
    store: &EventStore,
    projections: &Projections,
    agent_pool: &AgentPool,
    app: &AppHandle,
    session_id: AggregateId,
    event: ProviderEvent,
) -> Result<(), SupervisorError> {
    let now = Utc::now();

    // Capture a post-turn checkpoint on the terminal provider events, before
    // we touch the event store — we want the state as the provider left it.
    let post_turn = match &event {
        ProviderEvent::TurnCompleted { turn_id, .. }
        | ProviderEvent::TurnFailed { turn_id, .. } => Some(turn_id.clone()),
        _ => None,
    };
    if let Some(turn_id) = post_turn {
        let Some((root_path, environment)) = resolve_project(projections, session_id) else {
            tracing::debug!(%session_id, "no project resolved for checkpoint; skipping");
            return handle_provider_event_core(store, projections, app, session_id, event, now)
                .await;
        };
        let session_str = session_id.to_string();
        if let Err(e) = checkpoint::capture(
            &environment,
            agent_pool,
            &root_path,
            &session_str,
            &turn_id,
            Phase::Post,
        )
        .await
        {
            tracing::warn!(error = %e, %session_id, "post-turn checkpoint failed");
        }
    }

    handle_provider_event_core(store, projections, app, session_id, event, now).await
}

fn resolve_project(
    projections: &Projections,
    session_id: AggregateId,
) -> Option<(String, oxyris_core::Environment)> {
    let snap = projections.get_session(session_id).ok().flatten()?;
    let project_id = snap.data.project_id;
    let projects = projections.list_projects().ok()?;
    let p = projects.into_iter().find(|p| p.id == project_id)?;
    Some((p.root_path, p.environment))
}

async fn handle_provider_event_core(
    store: &EventStore,
    projections: &Projections,
    app: &AppHandle,
    session_id: AggregateId,
    event: ProviderEvent,
    now: chrono::DateTime<Utc>,
) -> Result<(), SupervisorError> {
    // Tool-approval prompts are transient UI state, not session history — we
    // surface them to the frontend on a side channel rather than persisting
    // them to the event log. The turn stays paused until the user answers via
    // `approve_tool_use` / `reject_tool_use`.
    if let ProviderEvent::ToolApprovalRequested {
        turn_id,
        request_id,
        tool_use_id,
        tool_name,
        input,
    } = &event
    {
        #[derive(Serialize, Clone)]
        struct ApprovalPayload<'a> {
            session_id: AggregateId,
            turn_id: &'a str,
            request_id: &'a str,
            tool_use_id: &'a str,
            tool_name: &'a str,
            input: &'a serde_json::Value,
        }
        let payload = ApprovalPayload {
            session_id,
            turn_id,
            request_id,
            tool_use_id,
            tool_name,
            input,
        };
        let _ = app.emit(&format!("session:{session_id}:approval"), payload);
        return Ok(());
    }

    let cmd = match event {
        ProviderEvent::SessionReady {
            provider_session_id: Some(provider_session_id),
            ..
        } => SessionCommand::AttachProviderSession {
            provider_session_id,
        },
        ProviderEvent::SessionReady { .. } => return Ok(()),
        ProviderEvent::AssistantBlock { turn_id, block } => {
            SessionCommand::AppendAssistantBlock { turn_id, block }
        }
        ProviderEvent::AssistantTextDelta { turn_id, text } => {
            SessionCommand::AppendAssistantBlock {
                turn_id,
                block: oxyris_provider::AssistantBlock::Text { text },
            }
        }
        ProviderEvent::TurnCompleted {
            turn_id,
            total_cost_usd,
            input_tokens,
            output_tokens,
        } => SessionCommand::CompleteTurn {
            turn_id,
            total_cost_usd,
            input_tokens,
            output_tokens,
            now,
        },
        ProviderEvent::TurnFailed { turn_id, message } => SessionCommand::FailTurn {
            turn_id,
            message,
            now,
        },
        // Handled by the early return above; here only to satisfy the match.
        ProviderEvent::ToolApprovalRequested { .. } => return Ok(()),
        ProviderEvent::SessionEnded => SessionCommand::Stop { now },
    };

    let stored = store.load(Session::KIND, session_id)?;
    let mut typed = Vec::with_capacity(stored.len());
    for s in &stored {
        let event: SessionEvent =
            serde_json::from_value(s.payload.clone()).map_err(EventStoreError::Serialization)?;
        typed.push(event);
    }
    let version = stored.last().map(|s| s.version).unwrap_or(0);
    let state = replay::<Session>(&typed);

    let events = match Session::decide(&state, cmd) {
        Ok(events) => events,
        Err(SessionError::TurnNotStreaming(_)) | Err(SessionError::UnknownTurn(_)) => {
            // Late events from a turn the user already interrupted — drop.
            return Ok(());
        }
        Err(e) => return Err(SupervisorError::Domain(e)),
    };
    if events.is_empty() {
        return Ok(());
    }
    let stored = store.append(Session::KIND, session_id, version, &events)?;
    for s in &stored {
        projections.apply(s)?;
    }
    for (s, ev) in stored.iter().zip(events.iter()) {
        let payload = EmittedSessionEvent {
            session_id,
            version: s.version,
            event: ev.clone(),
        };
        let event_name = format!("session:{session_id}:event");
        let _ = app.emit(&event_name, payload);
    }
    Ok(())
}
