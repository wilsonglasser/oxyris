//! Attachments storage for composer drag-drop / paste. The resulting absolute
//! path is handed back to the UI so it can prepend `@<path>` when sending the
//! message — Claude CLI resolves `@path` as a vision attachment.
//!
//! **Routing.** For Windows projects the file is written under
//! `<data_dir>/attachments/<bucket>/<uuid>.<ext>`. For WSL projects the bytes
//! are streamed through the per-distro agent (`fs.write_bytes`) into
//! `<home>/.oxyris/attachments/<bucket>/<uuid>.<ext>` *inside the distro*, and
//! the returned path is the POSIX one — a `\\wsl.localhost\…` or `C:\…` path
//! would be meaningless to a `claude` running within the distro.

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::op_name;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;

const MAX_SIZE: usize = 10 * 1024 * 1024; // 10 MB.

#[derive(Debug, Deserialize)]
pub struct AttachmentSaveInput {
    /// Folder bucket — the session id for attached files, or a transient
    /// `"pending-<uuid>"` for paste-before-session flows. Sanitized to prevent
    /// path traversal.
    pub bucket_id: String,
    /// Lower-case MIME type, e.g. `image/png`.
    pub mime: String,
    /// Base64-encoded bytes (no data-URL prefix).
    pub data_base64: String,
    /// Optional original filename; used only for extension detection fallback.
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AttachmentInfo {
    pub path: String,
    pub filename: String,
    pub mime: String,
    pub size: usize,
}

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriAttachmentError {
    #[error("unsupported mime: {0}")]
    UnsupportedMime(String),
    #[error("payload too large ({size} bytes, max {max})")]
    TooLarge { size: usize, max: usize },
    #[error("decode: {0}")]
    Decode(String),
    #[error("io: {0}")]
    Io(String),
    #[error("invalid bucket_id")]
    InvalidBucket,
    #[error("agent: {0}")]
    Agent(String),
}

#[tauri::command]
pub async fn attachment_save(
    input: AttachmentSaveInput,
    state: State<'_, AppState>,
) -> Result<AttachmentInfo, TauriAttachmentError> {
    let ext = extension_for(&input.mime, input.filename.as_deref())
        .ok_or_else(|| TauriAttachmentError::UnsupportedMime(input.mime.clone()))?;

    let bytes = B64
        .decode(input.data_base64.as_bytes())
        .map_err(|e| TauriAttachmentError::Decode(e.to_string()))?;
    if bytes.len() > MAX_SIZE {
        return Err(TauriAttachmentError::TooLarge {
            size: bytes.len(),
            max: MAX_SIZE,
        });
    }

    let bucket = sanitize_bucket(&input.bucket_id).ok_or(TauriAttachmentError::InvalidBucket)?;
    let filename = format!("{}.{ext}", Uuid::now_v7());
    let size = bytes.len();

    // Route by the bucket's project environment. Unresolvable buckets (e.g.
    // transient `pending-<uuid>`) fall through to the local Windows store.
    match resolve_environment(&state, &input.bucket_id) {
        Some(Environment::Wsl { distro }) => {
            let home = agent_home(&state, &distro).await?;
            let path = format!("{home}/.oxyris/attachments/{bucket}/{filename}");
            state
                .agent_pool
                .call(
                    &distro,
                    op_name::FS_WRITE_BYTES,
                    serde_json::json!({ "path": path, "bytes_b64": input.data_base64 }),
                )
                .await
                .map_err(|e| TauriAttachmentError::Agent(e.to_string()))?;
            Ok(AttachmentInfo {
                path,
                filename,
                mime: input.mime,
                size,
            })
        }
        _ => {
            let dir = state.data_dir.join("attachments").join(&bucket);
            std::fs::create_dir_all(&dir).map_err(|e| TauriAttachmentError::Io(e.to_string()))?;
            let path = dir.join(&filename);
            std::fs::write(&path, &bytes).map_err(|e| TauriAttachmentError::Io(e.to_string()))?;
            Ok(AttachmentInfo {
                path: path.display().to_string(),
                filename,
                mime: input.mime,
                size,
            })
        }
    }
}

/// Resolve the project environment for a bucket id. The bucket is a session id
/// for real attachments; transient `pending-<uuid>` buckets (and anything that
/// doesn't parse to a known session) return `None` so the caller stores locally.
fn resolve_environment(state: &AppState, bucket_id: &str) -> Option<Environment> {
    let session_id = AggregateId(uuid::Uuid::parse_str(bucket_id).ok()?);
    let snap = state.projections.get_session(session_id).ok()??;
    let projects = state.projections.list_projects().ok()?;
    projects
        .into_iter()
        .find(|p| p.id == snap.data.project_id)
        .map(|p| p.environment)
}

/// Ask the distro's agent for the home directory so attachments land somewhere
/// the agent's uid can write and `claude` can read (`<home>/.oxyris/...`).
async fn agent_home(state: &AppState, distro: &str) -> Result<String, TauriAttachmentError> {
    let info = state
        .agent_pool
        .call(distro, op_name::SYSTEM_INFO, serde_json::json!({}))
        .await
        .map_err(|e| TauriAttachmentError::Agent(e.to_string()))?;
    info.get("home")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| TauriAttachmentError::Agent("system.info returned no home".into()))
}

fn sanitize_bucket(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > 64 {
        return None;
    }
    if raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(raw.to_owned())
    } else {
        None
    }
}

fn extension_for(mime: &str, filename: Option<&str>) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => {
            // Fallback: infer from filename suffix for paste events that
            // don't set an explicit MIME.
            filename.and_then(|f| f.rsplit('.').next()).and_then(|ext| {
                match ext.to_ascii_lowercase().as_str() {
                    "png" => Some("png"),
                    "jpg" | "jpeg" => Some("jpg"),
                    "webp" => Some("webp"),
                    "gif" => Some("gif"),
                    _ => None,
                }
            })
        }
    }
}
