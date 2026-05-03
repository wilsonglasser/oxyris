//! Worktree-scoped filesystem ops for the file tree + editor UI.
//!
//! All public methods take a `(project_id, worktree_id, rel_path)` tuple.
//! `rel_path` is interpreted as a path **inside** the worktree root and is
//! rejected if it tries to escape via `..` or contains a drive letter / is
//! absolute. The dispatcher then routes per `Environment`:
//!
//! - `Windows` → `std::fs` directly (off the runtime via `spawn_blocking`).
//! - `Wsl { distro }` → agent ops (`fs.list_dir`, `fs.read`, `fs.write`).

use std::path::{Component, Path, PathBuf};

use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::{
    FsListDirArgs, FsListDirResult, FsReadArgs, FsReadResult, FsWriteArgs, FsWriteResult, op_name,
};
use thiserror::Error;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::infra::agent_pool::{AgentError, AgentPool};
use crate::infra::projections::ProjectionError;

/// Synthetic id for "the primary checkout" — matches
/// [`crate::tauri_commands::worktree::PRIMARY_WORKTREE_SENTINEL`].
const PRIMARY_WORKTREE_SENTINEL: AggregateId = AggregateId(Uuid::nil());

#[derive(Debug, Error)]
pub enum FsError {
    #[error("project not found")]
    ProjectNotFound,
    #[error("worktree not found")]
    WorktreeNotFound,
    #[error("worktree does not belong to project")]
    WorktreeProjectMismatch,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("agent: {0}")]
    Agent(String),
    #[error("projection: {0}")]
    Projection(String),
}

impl From<AgentError> for FsError {
    fn from(e: AgentError) -> Self {
        FsError::Agent(e.to_string())
    }
}

impl From<ProjectionError> for FsError {
    fn from(e: ProjectionError) -> Self {
        FsError::Projection(e.to_string())
    }
}

/// Resolve a `(project_id, worktree_id)` pair to its environment + absolute
/// root path on disk (Windows path or POSIX path inside the distro).
pub fn resolve_worktree(
    state: &AppState,
    project_id: AggregateId,
    worktree_id: AggregateId,
) -> Result<(Environment, String), FsError> {
    let project = state
        .projections
        .list_projects()?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or(FsError::ProjectNotFound)?;

    if worktree_id == PRIMARY_WORKTREE_SENTINEL {
        return Ok((project.environment, project.root_path));
    }
    let row = state
        .projections
        .get_worktree(worktree_id)?
        .ok_or(FsError::WorktreeNotFound)?;
    if row.project_id != project_id {
        return Err(FsError::WorktreeProjectMismatch);
    }
    Ok((project.environment, row.path))
}

/// Join `rel` onto `root` using the right separator for `env`, after
/// rejecting any segment that would escape the root or that ships its own
/// root (drive letter, leading slash). An empty `rel` returns `root`
/// unchanged.
pub fn join_inside_worktree(env: &Environment, root: &str, rel: &str) -> Result<String, FsError> {
    let trimmed = rel.trim_matches(['/', '\\']);
    if trimmed.is_empty() {
        return Ok(root.to_owned());
    }
    let parsed = Path::new(trimmed);
    if parsed.is_absolute() || trimmed.contains(':') {
        return Err(FsError::InvalidPath(format!("not relative: {rel}")));
    }
    let mut depth: i32 = 0;
    for comp in parsed.components() {
        match comp {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(FsError::InvalidPath(format!("escapes worktree: {rel}")));
                }
            }
            // RootDir / Prefix / etc. — already excluded by `is_absolute`,
            // but be defensive.
            _ => return Err(FsError::InvalidPath(format!("invalid component: {rel}"))),
        }
    }

    let sep = match env {
        Environment::Windows => '\\',
        Environment::Wsl { .. } => '/',
    };
    let normalized: Vec<String> = parsed
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let trim_root = root.trim_end_matches(['/', '\\']);
    let joined = normalized.join(&sep.to_string());
    Ok(format!("{trim_root}{sep}{joined}"))
}

// ────── ops ────────────────────────────────────────────────────────────────

pub async fn list_dir(
    env: &Environment,
    agent_pool: &AgentPool,
    abs_path: String,
    show_hidden: bool,
) -> Result<FsListDirResult, FsError> {
    match env {
        Environment::Windows => {
            tokio::task::spawn_blocking(move || list_dir_native(&abs_path, show_hidden))
                .await
                .map_err(|e| FsError::Agent(format!("join: {e}")))?
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::FS_LIST_DIR,
                    serde_json::to_value(FsListDirArgs {
                        path: abs_path,
                        show_hidden,
                    })
                    .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| FsError::Agent(e.to_string()))
        }
    }
}

pub async fn read_file(
    env: &Environment,
    agent_pool: &AgentPool,
    abs_path: String,
    max_bytes: Option<u64>,
) -> Result<FsReadResult, FsError> {
    match env {
        Environment::Windows => {
            tokio::task::spawn_blocking(move || read_file_native(&abs_path, max_bytes))
                .await
                .map_err(|e| FsError::Agent(format!("join: {e}")))?
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::FS_READ,
                    serde_json::to_value(FsReadArgs {
                        path: abs_path,
                        max_bytes,
                    })
                    .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| FsError::Agent(e.to_string()))
        }
    }
}

pub async fn write_file(
    env: &Environment,
    agent_pool: &AgentPool,
    abs_path: String,
    contents: String,
) -> Result<FsWriteResult, FsError> {
    match env {
        Environment::Windows => {
            tokio::task::spawn_blocking(move || write_file_native(&abs_path, &contents))
                .await
                .map_err(|e| FsError::Agent(format!("join: {e}")))?
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::FS_WRITE,
                    serde_json::to_value(FsWriteArgs {
                        path: abs_path,
                        contents,
                    })
                    .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| FsError::Agent(e.to_string()))
        }
    }
}

// ────── native impls (Windows projects) ────────────────────────────────────

const DEFAULT_READ_CAP: u64 = 1_048_576;

fn list_dir_native(path_str: &str, show_hidden: bool) -> Result<FsListDirResult, FsError> {
    use oxyris_ipc::ops::FsListDirEntry;
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(FsError::InvalidPath(format!("not found: {path_str}")));
    }
    let mut entries = Vec::new();
    for dent in std::fs::read_dir(path)? {
        let dent = dent?;
        let name = dent.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let ft = match dent.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let md = dent.metadata().ok();
        entries.push(FsListDirEntry {
            name,
            is_dir: ft.is_dir(),
            is_symlink: ft.is_symlink(),
            size: md
                .as_ref()
                .and_then(|m| if m.is_file() { Some(m.len()) } else { None }),
            modified_secs: md.as_ref().and_then(|m| {
                m.modified().ok().and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs() as i64)
                })
            }),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(FsListDirResult {
        path: path_str.to_owned(),
        entries,
    })
}

fn read_file_native(path_str: &str, max_bytes: Option<u64>) -> Result<FsReadResult, FsError> {
    use std::io::Read;
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(FsError::InvalidPath(format!("not found: {path_str}")));
    }
    let cap = max_bytes.unwrap_or(DEFAULT_READ_CAP);
    let mut file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    let mut limited = (&mut file).take(cap);
    limited.read_to_end(&mut buf)?;
    let bytes_read = buf.len() as u64;
    let metadata = file.metadata()?;
    let truncated = metadata.len() > bytes_read;
    let content = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    Ok(FsReadResult {
        path: path_str.to_owned(),
        content,
        bytes_read,
        truncated,
    })
}

fn write_file_native(path_str: &str, contents: &str) -> Result<FsWriteResult, FsError> {
    let path = PathBuf::from(path_str);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, contents)?;
    Ok(FsWriteResult {
        path: path_str.to_owned(),
        bytes_written: contents.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_rejects_parent_escape() {
        let err =
            join_inside_worktree(&Environment::Windows, r"C:\proj", r"..\..\evil").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath(_)));
    }

    #[test]
    fn join_allows_internal_parent() {
        let p =
            join_inside_worktree(&Environment::Windows, r"C:\proj", r"src\..\src\lib.rs").unwrap();
        assert_eq!(p, r"C:\proj\src\src\lib.rs");
    }

    #[test]
    fn join_rejects_drive_letter() {
        let err = join_inside_worktree(&Environment::Windows, r"C:\proj", r"D:\evil").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath(_)));
    }

    #[test]
    fn join_uses_posix_for_wsl() {
        let p = join_inside_worktree(
            &Environment::Wsl {
                distro: "Ubuntu".into(),
            },
            "/home/u/proj",
            "src/lib.rs",
        )
        .unwrap();
        assert_eq!(p, "/home/u/proj/src/lib.rs");
    }

    #[test]
    fn empty_rel_returns_root() {
        let p = join_inside_worktree(&Environment::Windows, r"C:\proj", "").unwrap();
        assert_eq!(p, r"C:\proj");
    }
}
