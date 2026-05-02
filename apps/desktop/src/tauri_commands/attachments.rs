//! Attachments storage for composer drag-drop / paste. Each file is written
//! under `<data_dir>/attachments/<session_id>/<uuid>.<ext>` and the resulting
//! absolute path is handed back to the UI so it can prepend `@<path>` when
//! sending the message — Claude CLI resolves `@path` as a vision attachment.

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
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
}

#[tauri::command]
pub fn attachment_save(
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
    let dir = state.data_dir.join("attachments").join(bucket);
    std::fs::create_dir_all(&dir).map_err(|e| TauriAttachmentError::Io(e.to_string()))?;

    let filename = format!("{}.{ext}", Uuid::now_v7());
    let path = dir.join(&filename);
    std::fs::write(&path, &bytes).map_err(|e| TauriAttachmentError::Io(e.to_string()))?;

    Ok(AttachmentInfo {
        path: path.display().to_string(),
        filename,
        mime: input.mime,
        size: bytes.len(),
    })
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
