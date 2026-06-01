//! Worktree-scoped filesystem ops for the file tree + editor UI.
//!
//! All public methods take a `(project_id, worktree_id, rel_path)` tuple.
//! `rel_path` is interpreted as a path **inside** the worktree root and is
//! rejected if it tries to escape via `..` or contains a drive letter / is
//! absolute. The dispatcher then routes per `Environment`:
//!
//! - `Local` → `std::fs` directly (off the runtime via `spawn_blocking`).
//! - `Wsl { distro }` → agent ops (`fs.list_dir`, `fs.read`, `fs.write`).

use std::path::{Component, Path, PathBuf};

use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::{
    FsContentFileHits, FsContentMatch, FsCopyArgs, FsCreateFileArgs, FsDeleteArgs, FsListDirArgs,
    FsListDirResult, FsPathArgs, FsReadArgs, FsReadBytesArgs, FsReadBytesResult, FsReadResult,
    FsRenameArgs, FsSearchContentArgs, FsSearchContentResult, FsSearchPathsArgs,
    FsSearchPathsResult, FsWriteArgs, FsWriteResult, op_name,
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
        // Host-native worktree path: follow the host OS separator (`\` on a
        // Windows host, `/` on macOS/Linux).
        Environment::Local => std::path::MAIN_SEPARATOR,
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
        Environment::Local => {
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
        Environment::Local => {
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
        Environment::Local => {
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

pub async fn create_file(
    env: &Environment,
    agent_pool: &AgentPool,
    abs_path: String,
    contents: String,
) -> Result<(), FsError> {
    match env {
        Environment::Local => tokio::task::spawn_blocking(move || -> Result<(), FsError> {
            let path = Path::new(&abs_path);
            if path.exists() {
                return Err(FsError::InvalidPath(format!("already exists: {abs_path}")));
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, &contents)?;
            Ok(())
        })
        .await
        .map_err(|e| FsError::Agent(format!("join: {e}")))?,
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::FS_CREATE_FILE,
                    serde_json::to_value(FsCreateFileArgs {
                        path: abs_path,
                        contents,
                    })
                    .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn create_dir(
    env: &Environment,
    agent_pool: &AgentPool,
    abs_path: String,
) -> Result<(), FsError> {
    match env {
        Environment::Local => tokio::task::spawn_blocking(move || -> Result<(), FsError> {
            std::fs::create_dir_all(&abs_path)?;
            Ok(())
        })
        .await
        .map_err(|e| FsError::Agent(format!("join: {e}")))?,
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::FS_CREATE_DIR,
                    serde_json::to_value(FsPathArgs { path: abs_path })
                        .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn rename(
    env: &Environment,
    agent_pool: &AgentPool,
    from: String,
    to: String,
) -> Result<(), FsError> {
    match env {
        Environment::Local => tokio::task::spawn_blocking(move || -> Result<(), FsError> {
            let from_p = Path::new(&from);
            let to_p = Path::new(&to);
            if !from_p.exists() {
                return Err(FsError::InvalidPath(format!("not found: {from}")));
            }
            if let Some(parent) = to_p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(from_p, to_p)?;
            Ok(())
        })
        .await
        .map_err(|e| FsError::Agent(format!("join: {e}")))?,
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::FS_RENAME,
                    serde_json::to_value(FsRenameArgs { from, to })
                        .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn copy(
    env: &Environment,
    agent_pool: &AgentPool,
    from: String,
    to: String,
) -> Result<(), FsError> {
    match env {
        Environment::Local => tokio::task::spawn_blocking(move || -> Result<(), FsError> {
            let from_p = Path::new(&from);
            let to_p = Path::new(&to);
            if !from_p.exists() {
                return Err(FsError::InvalidPath(format!("not found: {from}")));
            }
            if let Some(parent) = to_p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let md = std::fs::symlink_metadata(from_p)?;
            if md.is_dir() {
                copy_dir_recursive(from_p, to_p)?;
            } else {
                std::fs::copy(from_p, to_p)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| FsError::Agent(format!("join: {e}")))?,
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::FS_COPY,
                    serde_json::to_value(FsCopyArgs { from, to })
                        .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), FsError> {
    std::fs::create_dir_all(to)?;
    for dent in std::fs::read_dir(from)? {
        let dent = dent?;
        let src = dent.path();
        let dst = to.join(dent.file_name());
        if dent.file_type()?.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

pub async fn delete(
    env: &Environment,
    agent_pool: &AgentPool,
    abs_path: String,
    recursive: bool,
) -> Result<(), FsError> {
    match env {
        Environment::Local => tokio::task::spawn_blocking(move || -> Result<(), FsError> {
            let path = Path::new(&abs_path);
            if !path.exists() {
                return Err(FsError::InvalidPath(format!("not found: {abs_path}")));
            }
            let md = std::fs::symlink_metadata(path)?;
            if md.is_dir() {
                if recursive {
                    std::fs::remove_dir_all(path)?;
                } else {
                    std::fs::remove_dir(path)?;
                }
            } else {
                std::fs::remove_file(path)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| FsError::Agent(format!("join: {e}")))?,
        Environment::Wsl { distro } => {
            agent_pool
                .call(
                    distro,
                    op_name::FS_DELETE,
                    serde_json::to_value(FsDeleteArgs {
                        path: abs_path,
                        recursive,
                    })
                    .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            Ok(())
        }
    }
}

pub async fn read_bytes(
    env: &Environment,
    agent_pool: &AgentPool,
    abs_path: String,
    max_bytes: Option<u64>,
) -> Result<FsReadBytesResult, FsError> {
    match env {
        Environment::Local => {
            tokio::task::spawn_blocking(move || read_bytes_native(&abs_path, max_bytes))
                .await
                .map_err(|e| FsError::Agent(format!("join: {e}")))?
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::FS_READ_BYTES,
                    serde_json::to_value(FsReadBytesArgs {
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

pub async fn search_paths(
    env: &Environment,
    agent_pool: &AgentPool,
    root: String,
    query: String,
    limit: u32,
) -> Result<FsSearchPathsResult, FsError> {
    match env {
        Environment::Local => {
            let r = root.clone();
            let q = query.clone();
            tokio::task::spawn_blocking(move || search_paths_native(&r, &q, limit))
                .await
                .map_err(|e| FsError::Agent(format!("join: {e}")))?
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::FS_SEARCH_PATHS,
                    serde_json::to_value(FsSearchPathsArgs { root, query, limit })
                        .map_err(|e| FsError::Agent(e.to_string()))?,
                )
                .await?;
            serde_json::from_value(value).map_err(|e| FsError::Agent(e.to_string()))
        }
    }
}

pub async fn search_content(
    env: &Environment,
    agent_pool: &AgentPool,
    root: String,
    args: FsSearchContentArgs,
) -> Result<FsSearchContentResult, FsError> {
    match env {
        Environment::Local => {
            let a = FsSearchContentArgs { root, ..args };
            tokio::task::spawn_blocking(move || search_content_native(&a))
                .await
                .map_err(|e| FsError::Agent(format!("join: {e}")))?
        }
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::FS_SEARCH_CONTENT,
                    serde_json::to_value(FsSearchContentArgs { root, ..args })
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
        // Only `.git` is hidden by default; other dotfiles (.claude, .env, …)
        // stay visible. `show_hidden` reveals `.git` too.
        if !show_hidden && name == ".git" {
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

fn read_bytes_native(path_str: &str, max_bytes: Option<u64>) -> Result<FsReadBytesResult, FsError> {
    use base64::Engine;
    use std::io::Read;
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(FsError::InvalidPath(format!("not found: {path_str}")));
    }
    let cap = max_bytes.unwrap_or(16 * 1024 * 1024);
    let mut file = std::fs::File::open(path)?;
    let total = file.metadata()?.len();
    let mut buf = Vec::with_capacity(cap.min(total) as usize);
    (&mut file).take(cap).read_to_end(&mut buf)?;
    let bytes_read = buf.len() as u64;
    let bytes_b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(FsReadBytesResult {
        path: path_str.to_owned(),
        bytes_b64,
        bytes_read,
        truncated: total > bytes_read,
    })
}

fn search_paths_native(
    root_str: &str,
    query: &str,
    limit: u32,
) -> Result<FsSearchPathsResult, FsError> {
    use ignore::WalkBuilder;
    use oxyris_ipc::ops::FsSearchHit;
    let root = Path::new(root_str);
    if !root.exists() {
        return Err(FsError::InvalidPath(format!("not found: {root_str}")));
    }
    let q_lower = query.to_lowercase();
    let mut hits: Vec<FsSearchHit> = Vec::new();
    let mut walked = 0u32;
    let mut truncated = false;
    const WALK_CAP: u32 = 20_000;

    for dent in WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(false)
        .follow_links(false)
        .build()
    {
        let Ok(dent) = dent else { continue };
        walked += 1;
        if walked > WALK_CAP {
            truncated = true;
            break;
        }
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(rel) = dent.path().strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if oxyris_ipc::ops::is_generated_path(&rel_str) {
            continue;
        }
        if !q_lower.is_empty() {
            let hay = rel_str.to_lowercase();
            if !hay.contains(&q_lower) {
                continue;
            }
            let basename = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let in_base = basename.to_lowercase().contains(&q_lower);
            let depth = rel.components().count() as i32;
            let score = if in_base { depth } else { depth + 100 };
            hits.push(FsSearchHit {
                rel_path: rel_str,
                score,
            });
        } else {
            hits.push(FsSearchHit {
                rel_path: rel_str,
                score: rel.components().count() as i32,
            });
        }
    }

    hits.sort_by(|a, b| {
        a.score
            .cmp(&b.score)
            .then_with(|| a.rel_path.len().cmp(&b.rel_path.len()))
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    let cap = limit as usize;
    if hits.len() > cap {
        hits.truncate(cap);
        truncated = true;
    }
    Ok(FsSearchPathsResult { hits, truncated })
}

// ────── content search (Find in Files) ────────────────────────────────────

const SEARCH_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const SEARCH_WALK_CAP: u32 = 50_000;
const SEARCH_MAX_LINE_LEN: usize = 500;

fn build_content_matcher(args: &FsSearchContentArgs) -> Result<regex::Regex, String> {
    if args.query.is_empty() {
        return Err("empty query".to_owned());
    }
    let base = if args.is_regex {
        args.query.clone()
    } else {
        regex::escape(&args.query)
    };
    let pat = if args.whole_word {
        format!(r"\b(?:{base})\b")
    } else {
        base
    };
    regex::RegexBuilder::new(&pat)
        .case_insensitive(!args.case_sensitive)
        .build()
        .map_err(|e| e.to_string())
}

fn cap_line(line: &str) -> String {
    if line.len() <= SEARCH_MAX_LINE_LEN {
        return line.to_owned();
    }
    let mut end = SEARCH_MAX_LINE_LEN;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

fn search_content_native(args: &FsSearchContentArgs) -> Result<FsSearchContentResult, FsError> {
    use ignore::WalkBuilder;
    use ignore::overrides::OverrideBuilder;

    let root = Path::new(&args.root);
    if !root.exists() {
        return Err(FsError::InvalidPath(format!("not found: {}", args.root)));
    }
    let re = build_content_matcher(args).map_err(FsError::InvalidPath)?;

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(false)
        .follow_links(false);
    if let Some(mask) = args.include_glob.as_deref() {
        let mut ob = OverrideBuilder::new(root);
        for pat in mask.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let _ = ob.add(pat);
        }
        if let Ok(ov) = ob.build() {
            builder.overrides(ov);
        }
    }

    let max_results = args.max_results.max(1);
    let mut files: Vec<FsContentFileHits> = Vec::new();
    let mut total = 0u32;
    let mut truncated = false;
    let mut walked = 0u32;

    for dent in builder.build() {
        let Ok(dent) = dent else { continue };
        walked += 1;
        if walked > SEARCH_WALK_CAP {
            truncated = true;
            break;
        }
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(md) = dent.metadata()
            && md.len() > SEARCH_MAX_FILE_BYTES
        {
            continue;
        }
        let path = dent.path();
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.iter().take(8000).any(|&b| b == 0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        let mut matches = Vec::new();
        let mut hit_cap = false;
        for (i, line) in text.lines().enumerate() {
            if total >= max_results {
                hit_cap = true;
                break;
            }
            if re.is_match(line) {
                matches.push(FsContentMatch {
                    line: (i as u32) + 1,
                    text: cap_line(line),
                });
                total += 1;
            }
        }
        if !matches.is_empty() {
            files.push(FsContentFileHits {
                rel_path: rel_str,
                matches,
            });
        }
        if hit_cap {
            truncated = true;
            break;
        }
    }

    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(FsSearchContentResult {
        files,
        total_matches: total,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Windows-host Local semantics: backslash separators + drive letters.
    #[cfg(windows)]
    #[test]
    fn join_rejects_parent_escape() {
        let err = join_inside_worktree(&Environment::Local, r"C:\proj", r"..\..\evil").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath(_)));
    }

    #[cfg(windows)]
    #[test]
    fn join_allows_internal_parent() {
        let p =
            join_inside_worktree(&Environment::Local, r"C:\proj", r"src\..\src\lib.rs").unwrap();
        assert_eq!(p, r"C:\proj\src\src\lib.rs");
    }

    #[cfg(windows)]
    #[test]
    fn join_rejects_drive_letter() {
        let err = join_inside_worktree(&Environment::Local, r"C:\proj", r"D:\evil").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath(_)));
    }

    // Unix-host Local semantics: forward-slash separators.
    #[cfg(not(windows))]
    #[test]
    fn join_rejects_parent_escape_posix() {
        let err =
            join_inside_worktree(&Environment::Local, "/home/u/proj", "../../evil").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath(_)));
    }

    #[cfg(not(windows))]
    #[test]
    fn join_allows_internal_parent_posix() {
        let p =
            join_inside_worktree(&Environment::Local, "/home/u/proj", "src/../src/lib.rs").unwrap();
        assert_eq!(p, "/home/u/proj/src/src/lib.rs");
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
        let p = join_inside_worktree(&Environment::Local, r"C:\proj", "").unwrap();
        assert_eq!(p, r"C:\proj");
    }
}
