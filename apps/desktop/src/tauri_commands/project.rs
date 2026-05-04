//! Tauri IPC surface for the Project aggregate.
//!
//! Handlers here orchestrate one write cycle:
//!
//! 1. load current state via `replay` on the event log,
//! 2. call the pure `decide`,
//! 3. append the produced events under the current version (optimistic
//!    concurrency),
//! 4. apply each stored event to the projection so the UI's next `project_list`
//!    sees it without waiting for a rebuild.

use chrono::Utc;
use oxyris_core::{Aggregate, AggregateId, Environment, replay};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::domain::project::{Project, ProjectCommand, ProjectError, ProjectEvent, ProjectState};
use crate::infra::event_store::EventStoreError;
use crate::infra::projections::{ProjectRow, ProjectionError};

#[derive(Debug, Serialize)]
pub struct ProjectCreateResponse {
    pub id: AggregateId,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectInput {
    pub name: String,
    pub environment: Environment,
    pub root_path: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameProjectInput {
    pub id: AggregateId,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteProjectInput {
    pub id: AggregateId,
}

/// A Tauri-facing error. We flatten our internal errors into a discriminated
/// string + optional message so the web can show friendly copy via i18n keys.
#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriProjectError {
    #[error("domain: {0}")]
    Domain(String),
    #[error("concurrency")]
    Concurrency,
    #[error("storage: {0}")]
    Storage(String),
    #[error("projection: {0}")]
    Projection(String),
}

impl From<ProjectError> for TauriProjectError {
    fn from(e: ProjectError) -> Self {
        TauriProjectError::Domain(e.to_string())
    }
}

impl From<EventStoreError> for TauriProjectError {
    fn from(e: EventStoreError) -> Self {
        match e {
            EventStoreError::Concurrency { .. } => TauriProjectError::Concurrency,
            other => TauriProjectError::Storage(other.to_string()),
        }
    }
}

impl From<ProjectionError> for TauriProjectError {
    fn from(e: ProjectionError) -> Self {
        TauriProjectError::Projection(e.to_string())
    }
}

fn load_state(state: &AppState, id: AggregateId) -> Result<(ProjectState, u32), TauriProjectError> {
    let stored = state.event_store.load(Project::KIND, id)?;
    let mut typed = Vec::with_capacity(stored.len());
    for s in &stored {
        let event: ProjectEvent = serde_json::from_value(s.payload.clone())
            .map_err(|e| TauriProjectError::Storage(format!("payload decode: {e}")))?;
        typed.push(event);
    }
    let version = stored.last().map(|s| s.version).unwrap_or(0);
    Ok((replay::<Project>(&typed), version))
}

fn dispatch(
    state: &AppState,
    id: AggregateId,
    current_version: u32,
    events: Vec<ProjectEvent>,
) -> Result<(), TauriProjectError> {
    if events.is_empty() {
        return Ok(());
    }
    let stored = state
        .event_store
        .append(Project::KIND, id, current_version, &events)?;
    for s in &stored {
        state.projections.apply(s)?;
    }
    Ok(())
}

#[tauri::command]
pub fn project_create(
    input: CreateProjectInput,
    state: State<'_, AppState>,
) -> Result<ProjectCreateResponse, TauriProjectError> {
    let id = AggregateId::new();
    let cmd = ProjectCommand::Create {
        id,
        name: input.name,
        environment: input.environment,
        root_path: input.root_path,
        now: Utc::now(),
    };
    // A fresh aggregate starts at version 0.
    let events = Project::decide(&ProjectState::default(), cmd)?;
    dispatch(&state, id, 0, events)?;
    Ok(ProjectCreateResponse { id })
}

#[tauri::command]
pub fn project_rename(
    input: RenameProjectInput,
    state: State<'_, AppState>,
) -> Result<(), TauriProjectError> {
    let (project_state, version) = load_state(&state, input.id)?;
    let events = Project::decide(
        &project_state,
        ProjectCommand::Rename {
            new_name: input.new_name,
        },
    )?;
    dispatch(&state, input.id, version, events)
}

#[tauri::command]
pub fn project_delete(
    input: DeleteProjectInput,
    state: State<'_, AppState>,
) -> Result<(), TauriProjectError> {
    let (project_state, version) = load_state(&state, input.id)?;
    let events = Project::decide(&project_state, ProjectCommand::Delete)?;
    dispatch(&state, input.id, version, events)
}

#[tauri::command]
pub fn project_list(state: State<'_, AppState>) -> Result<Vec<ProjectRow>, TauriProjectError> {
    Ok(state.projections.list_projects()?)
}

// ────── logo (set / autodetect / read bytes) ───────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetProjectLogoInput {
    pub id: AggregateId,
    /// Path stored on the project. Either an absolute path (Windows: drive
    /// letter; WSL: POSIX) or a path relative to the project root. `None`
    /// clears any existing logo override.
    #[serde(default)]
    pub logo_path: Option<String>,
}

#[tauri::command]
pub fn project_set_logo(
    input: SetProjectLogoInput,
    state: State<'_, AppState>,
) -> Result<(), TauriProjectError> {
    let (project_state, version) = load_state(&state, input.id)?;
    let events = Project::decide(
        &project_state,
        ProjectCommand::SetLogo {
            logo_path: input.logo_path,
        },
    )?;
    dispatch(&state, input.id, version, events)
}

#[derive(Debug, Deserialize)]
pub struct AutodetectLogoInput {
    pub id: AggregateId,
}

#[derive(Debug, Serialize)]
pub struct AutodetectLogoOutput {
    /// Suggested path relative to the project root, or `None` when no
    /// candidate was found. Caller decides whether to persist via
    /// `project_set_logo`.
    pub logo_path: Option<String>,
}

#[tauri::command]
pub fn project_autodetect_logo(
    input: AutodetectLogoInput,
    state: State<'_, AppState>,
) -> Result<AutodetectLogoOutput, TauriProjectError> {
    let project = state
        .projections
        .list_projects()?
        .into_iter()
        .find(|p| p.id == input.id)
        .ok_or_else(|| TauriProjectError::Domain("project not found".into()))?;
    // Windows projects scan natively; WSL projects use UNC translation
    // (UX path only — the autodetect runs once per click, fast enough).
    let root = std::path::PathBuf::from(unc_for_root(&project.environment, &project.root_path));
    let logo_path = autodetect_logo_under(&root);
    Ok(AutodetectLogoOutput { logo_path })
}

fn unc_for_root(env: &Environment, root: &str) -> String {
    match env {
        Environment::Windows => root.to_owned(),
        Environment::Wsl { distro } => {
            // Render the POSIX path as a Windows-side UNC so std::fs can
            // walk it. Hot-path ops still go through the agent — this is
            // the documented exception (PLAN.md §13: one-shot UX is OK
            // over UNC).
            let trimmed = root.trim_start_matches('/');
            format!(
                "\\\\wsl.localhost\\{distro}\\{}",
                trimmed.replace('/', "\\")
            )
        }
    }
}

const LOGO_NAMES: &[&str] = &["logo", "icon", "brand", "oxyris", "favicon"];
const LOGO_EXTS: &[&str] = &["svg", "png", "webp", "jpg", "jpeg", "ico"];
const LOGO_DIRS: &[&str] = &["", "assets", "public", "static", "docs", ".github"];

fn autodetect_logo_under(root: &std::path::Path) -> Option<String> {
    for dir in LOGO_DIRS {
        let base = if dir.is_empty() {
            root.to_path_buf()
        } else {
            root.join(dir)
        };
        if !base.exists() {
            continue;
        }
        for name in LOGO_NAMES {
            for ext in LOGO_EXTS {
                let candidate = base.join(format!("{name}.{ext}"));
                if candidate.is_file() {
                    let rel = candidate.strip_prefix(root).unwrap_or(&candidate);
                    return Some(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    None
}

#[derive(Debug, Deserialize)]
pub struct ReadLogoBytesInput {
    pub id: AggregateId,
}

#[derive(Debug, Serialize)]
pub struct ReadLogoBytesOutput {
    pub bytes_b64: String,
    pub mime: String,
}

#[tauri::command]
pub async fn project_logo_bytes(
    input: ReadLogoBytesInput,
    state: State<'_, AppState>,
) -> Result<Option<ReadLogoBytesOutput>, TauriProjectError> {
    use base64::Engine;
    let project = state
        .projections
        .list_projects()?
        .into_iter()
        .find(|p| p.id == input.id)
        .ok_or_else(|| TauriProjectError::Domain("project not found".into()))?;
    let Some(rel) = project.logo_path.as_deref() else {
        return Ok(None);
    };
    let abs = if std::path::Path::new(rel).is_absolute() {
        std::path::PathBuf::from(rel)
    } else {
        std::path::PathBuf::from(unc_for_root(&project.environment, &project.root_path)).join(rel)
    };
    let abs_clone = abs.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&abs_clone))
        .await
        .map_err(|e| TauriProjectError::Storage(format!("join: {e}")))?
        .map_err(|e| TauriProjectError::Storage(e.to_string()))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let bytes_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mime = mime_for_logo(&abs);
    Ok(Some(ReadLogoBytesOutput { bytes_b64, mime }))
}

fn mime_for_logo(p: &std::path::Path) -> String {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
    .to_owned()
}
