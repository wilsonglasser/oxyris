//! Editor-facing LSP commands: the file editor's diagnostics, hover,
//! go-to-definition and format actions.
//!
//! These share the per-worktree servers `LspManager` already runs for the MCP
//! tools — one rust-analyzer / tsserver / intelephense per worktree, whoever
//! asks first. Nothing here spawns anything of its own.
//!
//! **Text comes from the caller, never from disk.** The editor's buffer is
//! unsaved by definition, and for WSL projects the paths are POSIX paths
//! *inside the distro* which the Windows-side process cannot read at all. So
//! every command takes the live buffer and pushes it with `open_or_update`
//! before asking a question about it.
//!
//! Every command resolves `(project_id, worktree_id, rel_path)` the same way
//! the `fs` commands do, so path safety and Windows/WSL routing are identical.
//! A missing language server is a soft failure: the frontend drops the feature
//! for that file and the editor stays usable.

use std::path::{Path, PathBuf};

use oxyris_core::{AggregateId, Environment};
use oxyris_lsp::LspLanguage;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::infra::fs::{self as fs_infra};
use crate::infra::lsp::LspManager;

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriLspError {
    /// No language server covers this file (unsupported extension, or the
    /// workspace has none of the languages we support). Not an error the user
    /// needs to see — the editor just skips LSP for that tab.
    #[error("no language server for this file")]
    Unsupported,
    #[error("{0}")]
    Backend(String),
}

/// One diagnostic, flattened for the frontend. Positions are 0-based and
/// `character` counts UTF-16 code units — the same units a JS string index
/// uses, so the editor can map them without conversion.
#[derive(Debug, Serialize)]
pub struct LspDiagnostic {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
    /// `"error" | "warning" | "info" | "hint"`.
    pub severity: String,
    pub message: String,
    /// Producing tool, when the server reports one (`rustc`, `clippy`, …).
    pub source: Option<String>,
}

/// A definition target. `rel_path` is set when the target lives inside the
/// worktree (the editor can open it); otherwise only `abs_path` is — think
/// `~/.cargo/registry` or a `node_modules` d.ts.
#[derive(Debug, Serialize)]
pub struct LspLocation {
    pub rel_path: Option<String>,
    pub abs_path: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Deserialize)]
pub struct LspDocInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    /// Live editor buffer. Pushed to the server before the query so answers
    /// describe what the user is looking at, not what was last saved.
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct LspPositionInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    pub text: String,
    /// 0-based, LSP-native.
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Deserialize)]
pub struct LspCloseInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
}

#[derive(Debug, Deserialize)]
pub struct LspFormatInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    pub text: String,
    pub tab_size: u32,
    pub insert_spaces: bool,
}

/// A formatting edit, in the same 0-based line/character coordinates as
/// [`LspDiagnostic`]. Offsets are against the text that was submitted.
#[derive(Debug, Serialize)]
pub struct LspTextEdit {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub new_text: String,
}

/// Resolve the worktree, the absolute file path and the language server that
/// covers the file. `Unsupported` means "no LSP for this tab" — the caller
/// treats it as a no-op, not a failure.
async fn resolve(
    state: &State<'_, AppState>,
    project_id: AggregateId,
    worktree_id: AggregateId,
    rel_path: &str,
) -> Result<(PathBuf, PathBuf, Environment, LspLanguage), TauriLspError> {
    let (env, root) = fs_infra::resolve_worktree(state, project_id, worktree_id)
        .map_err(|e| TauriLspError::Backend(e.to_string()))?;
    let abs = fs_infra::safe_join(&env, &root, rel_path)
        .map_err(|e| TauriLspError::Backend(e.to_string()))?;
    let root_path = PathBuf::from(&root);
    let abs_path = PathBuf::from(&abs);
    let lang = LspManager::language_for_workspace(&root_path, &abs_path)
        .ok_or(TauriLspError::Unsupported)?;
    Ok((root_path, abs_path, env, lang))
}

/// Push the buffer and return the client, ready to be queried.
async fn client_with_buffer(
    state: &State<'_, AppState>,
    root: &Path,
    abs: &Path,
    env: &Environment,
    lang: LspLanguage,
    text: &str,
) -> Result<std::sync::Arc<oxyris_lsp::LspClient>, TauriLspError> {
    let client = state
        .lsp
        .ensure_at(root, env, lang)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))?;
    client
        .open_or_update(abs, text)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))?;
    Ok(client)
}

/// Sync the buffer and return whatever the server has published for it.
///
/// Diagnostics arrive asynchronously (`publishDiagnostics`), so a call made
/// right after an edit returns the *previous* pass. The frontend re-polls;
/// there is no request/response form of this in LSP.
#[tauri::command]
pub async fn lsp_diagnostics(
    input: LspDocInput,
    state: State<'_, AppState>,
) -> Result<Vec<LspDiagnostic>, TauriLspError> {
    let (root, abs, env, lang) =
        resolve(&state, input.project_id, input.worktree_id, &input.rel_path).await?;
    let client = client_with_buffer(&state, &root, &abs, &env, lang, &input.text).await?;
    let diags = client
        .diagnostics_for(&abs)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))?;
    Ok(diags.into_iter().map(flatten_diagnostic).collect())
}

/// Hover documentation at a position. `None` when the server has nothing.
#[tauri::command]
pub async fn lsp_hover(
    input: LspPositionInput,
    state: State<'_, AppState>,
) -> Result<Option<String>, TauriLspError> {
    let (root, abs, env, lang) =
        resolve(&state, input.project_id, input.worktree_id, &input.rel_path).await?;
    let client = client_with_buffer(&state, &root, &abs, &env, lang, &input.text).await?;
    client
        .hover(&abs, input.line, input.character)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))
}

/// Go to definition. Returns every candidate the server offers, worktree-local
/// ones first — those are the only ones the editor can open in a tab.
#[tauri::command]
pub async fn lsp_definition(
    input: LspPositionInput,
    state: State<'_, AppState>,
) -> Result<Vec<LspLocation>, TauriLspError> {
    let (root, abs, env, lang) =
        resolve(&state, input.project_id, input.worktree_id, &input.rel_path).await?;
    let client = client_with_buffer(&state, &root, &abs, &env, lang, &input.text).await?;
    let locations = client
        .definition(&abs, input.line, input.character)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))?;
    let mut out: Vec<LspLocation> = locations
        .into_iter()
        .map(|loc| location_from_uri(&root, &loc))
        .collect();
    out.sort_by_key(|l| l.rel_path.is_none());
    Ok(out)
}

/// Format the whole document. The edits are against `text` as submitted.
#[tauri::command]
pub async fn lsp_format(
    input: LspFormatInput,
    state: State<'_, AppState>,
) -> Result<Vec<LspTextEdit>, TauriLspError> {
    let (root, abs, env, lang) =
        resolve(&state, input.project_id, input.worktree_id, &input.rel_path).await?;
    let client = client_with_buffer(&state, &root, &abs, &env, lang, &input.text).await?;
    let edits = client
        .formatting(&abs, input.tab_size, input.insert_spaces)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))?;
    Ok(edits
        .into_iter()
        .map(|e| LspTextEdit {
            start_line: e.range.start.line,
            start_character: e.range.start.character,
            end_line: e.range.end.line,
            end_character: e.range.end.character,
            new_text: e.new_text,
        })
        .collect())
}

/// Notify the server that the user saved. Kept apart from the per-keystroke
/// sync because save is what triggers the check layer (rust-analyzer runs
/// `cargo check` on it) — sending it per edit would spin a build loop.
#[tauri::command]
pub async fn lsp_did_save(
    input: LspDocInput,
    state: State<'_, AppState>,
) -> Result<(), TauriLspError> {
    let (root, abs, env, lang) =
        resolve(&state, input.project_id, input.worktree_id, &input.rel_path).await?;
    let client = client_with_buffer(&state, &root, &abs, &env, lang, &input.text).await?;
    client
        .did_save(&abs)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))
}

/// Editor tab closed — let the server drop our buffer and go back to disk.
/// Never spawns a server: if none is running for this worktree there is
/// nothing holding the document either.
#[tauri::command]
pub async fn lsp_did_close(
    input: LspCloseInput,
    state: State<'_, AppState>,
) -> Result<(), TauriLspError> {
    let (root, abs, env, lang) =
        resolve(&state, input.project_id, input.worktree_id, &input.rel_path).await?;
    let Some(client) = state.lsp.running_at(&root, &env, lang).await else {
        return Ok(());
    };
    client
        .close_document(&abs)
        .await
        .map_err(|e| TauriLspError::Backend(e.to_string()))
}

fn flatten_diagnostic(d: oxyris_lsp::lsp_types::Diagnostic) -> LspDiagnostic {
    use oxyris_lsp::lsp_types::DiagnosticSeverity;
    let severity = match d.severity {
        Some(DiagnosticSeverity::ERROR) | None => "error",
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "info",
        _ => "hint",
    };
    LspDiagnostic {
        start_line: d.range.start.line,
        start_character: d.range.start.character,
        end_line: d.range.end.line,
        end_character: d.range.end.character,
        severity: severity.to_owned(),
        message: d.message,
        source: d.source,
    }
}

/// Turn a `file:` URI back into paths the editor can use. `rel_path` is only
/// filled when the target sits inside the worktree.
fn location_from_uri(root: &Path, loc: &oxyris_lsp::lsp_types::Location) -> LspLocation {
    let raw = loc.uri.as_str();
    let decoded = percent_decode(raw.strip_prefix("file://").unwrap_or(raw));
    // `file:///C:/…` leaves a leading slash before the drive letter.
    let trimmed = if decoded.len() > 2 && decoded.starts_with('/') && decoded.as_bytes()[2] == b':'
    {
        decoded[1..].to_owned()
    } else {
        decoded
    };
    let normalized = trimmed.replace('\\', "/");
    let root_norm = root.to_string_lossy().replace('\\', "/");
    let rel = normalized
        .strip_prefix(&format!("{}/", root_norm.trim_end_matches('/')))
        .map(str::to_owned);
    LspLocation {
        rel_path: rel,
        abs_path: normalized,
        line: loc.range.start.line,
        character: loc.range.start.character,
    }
}

/// Minimal `%XX` decoder — enough for the space/`:` escaping servers apply to
/// `file:` URIs. Invalid escapes are left as-is rather than dropped.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxyris_lsp::lsp_types::{Location, Position, Range, Uri};

    fn loc(uri: &str) -> Location {
        Location {
            uri: uri.parse::<Uri>().expect("uri"),
            range: Range {
                start: Position {
                    line: 7,
                    character: 4,
                },
                end: Position {
                    line: 7,
                    character: 9,
                },
            },
        }
    }

    #[test]
    fn worktree_local_target_gets_a_relative_path() {
        let out = location_from_uri(
            Path::new("C:\\proj\\app"),
            &loc("file:///C:/proj/app/src/main.rs"),
        );
        assert_eq!(out.rel_path.as_deref(), Some("src/main.rs"));
        assert_eq!(out.line, 7);
        assert_eq!(out.character, 4);
    }

    #[test]
    fn target_outside_the_worktree_has_no_relative_path() {
        let out = location_from_uri(
            Path::new("C:\\proj\\app"),
            &loc("file:///C:/Users/x/.cargo/registry/serde/lib.rs"),
        );
        assert!(out.rel_path.is_none());
        assert!(out.abs_path.ends_with("serde/lib.rs"));
    }

    #[test]
    fn posix_wsl_paths_round_trip() {
        let out = location_from_uri(
            Path::new("/home/w/sis"),
            &loc("file:///home/w/sis/app/Models/User.php"),
        );
        assert_eq!(out.rel_path.as_deref(), Some("app/Models/User.php"));
    }

    #[test]
    fn percent_escapes_are_decoded() {
        let out = location_from_uri(
            Path::new("/home/w/my proj"),
            &loc("file:///home/w/my%20proj/src/a.rs"),
        );
        assert_eq!(out.rel_path.as_deref(), Some("src/a.rs"));
    }
}
