//! Worktree-scoped filesystem commands powering the file tree + editor UI.
//!
//! Every command takes `(project_id, worktree_id, rel_path)`. Path safety
//! (no escape via `..`, no absolute paths, no drive letters) is enforced in
//! `infra::fs::join_inside_worktree`. Windows projects hit `std::fs`
//! directly; WSL projects route through the agent.

use oxyris_core::AggregateId;
use oxyris_ipc::ops::{FsListDirResult, FsReadResult, FsWriteResult};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::app_state::AppState;
use crate::infra::fs::{self as fs_infra, FsError};

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum TauriFsError {
    #[error("{0}")]
    Backend(String),
}

impl From<FsError> for TauriFsError {
    fn from(e: FsError) -> Self {
        TauriFsError::Backend(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct FsListDirInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    #[serde(default)]
    pub rel_path: String,
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Debug, Serialize)]
pub struct FsListDirOutput {
    /// Absolute path of the listed directory (Windows path or POSIX path
    /// inside the distro). Returned so the frontend can show it in a
    /// breadcrumb without re-deriving it.
    pub abs_path: String,
    pub entries: Vec<oxyris_ipc::ops::FsListDirEntry>,
}

#[tauri::command]
pub async fn fs_list_dir(
    input: FsListDirInput,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<FsListDirOutput, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    // Lazy-install the per-worktree fs watcher on first listing so the
    // tree refreshes automatically when files change on disk. Idempotent
    // and silently no-op'd for WSL projects.
    state
        .fs_watcher
        .ensure(
            app.clone(),
            input.worktree_id,
            &env,
            root.clone(),
            state.indexing.clone(),
        )
        .await;
    // WSL counterpart — native inotify inside the distro via the agent. Also
    // idempotent; no-op'd for Windows projects.
    state
        .wsl_fs_watcher
        .ensure(app.clone(), input.worktree_id, &env, root.clone())
        .await;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    let result: FsListDirResult =
        fs_infra::list_dir(&env, &state.agent_pool, abs.clone(), input.show_hidden).await?;
    Ok(FsListDirOutput {
        abs_path: result.path,
        entries: result.entries,
    })
}

#[derive(Debug, Deserialize)]
pub struct FsReadFileInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    /// Cap on bytes; defaults to 1 MiB to keep huge binaries from blowing up
    /// the editor.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct FsReadFileOutput {
    pub abs_path: String,
    pub content: String,
    pub bytes_read: u64,
    pub truncated: bool,
}

#[tauri::command]
pub async fn fs_read_file(
    input: FsReadFileInput,
    state: State<'_, AppState>,
) -> Result<FsReadFileOutput, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    let result: FsReadResult =
        fs_infra::read_file(&env, &state.agent_pool, abs, input.max_bytes).await?;
    Ok(FsReadFileOutput {
        abs_path: result.path,
        content: result.content,
        bytes_read: result.bytes_read,
        truncated: result.truncated,
    })
}

#[derive(Debug, Deserialize)]
pub struct FsWriteFileInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct FsWriteFileOutput {
    pub abs_path: String,
    pub bytes_written: u64,
}

#[tauri::command]
pub async fn fs_write_file(
    input: FsWriteFileInput,
    state: State<'_, AppState>,
) -> Result<FsWriteFileOutput, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    let result: FsWriteResult =
        fs_infra::write_file(&env, &state.agent_pool, abs, input.content).await?;
    Ok(FsWriteFileOutput {
        abs_path: result.path,
        bytes_written: result.bytes_written,
    })
}

// ────── file ops (create / rename / delete) ───────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FsCreateInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    #[serde(default)]
    pub contents: String,
}

#[tauri::command]
pub async fn fs_create_file(
    input: FsCreateInput,
    state: State<'_, AppState>,
) -> Result<(), TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    fs_infra::create_file(&env, &state.agent_pool, abs, input.contents).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct FsCreateDirInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
}

#[tauri::command]
pub async fn fs_create_dir(
    input: FsCreateDirInput,
    state: State<'_, AppState>,
) -> Result<(), TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    fs_infra::create_dir(&env, &state.agent_pool, abs).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct FsRenameInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub from_rel: String,
    pub to_rel: String,
}

#[tauri::command]
pub async fn fs_rename(
    input: FsRenameInput,
    state: State<'_, AppState>,
) -> Result<(), TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let from = fs_infra::safe_join(&env, &root, &input.from_rel)?;
    let to = fs_infra::safe_join(&env, &root, &input.to_rel)?;
    fs_infra::rename(&env, &state.agent_pool, from, to).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct FsDeleteInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[tauri::command]
pub async fn fs_delete(
    input: FsDeleteInput,
    state: State<'_, AppState>,
) -> Result<(), TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    fs_infra::delete(&env, &state.agent_pool, abs, input.recursive).await?;
    Ok(())
}

// ────── copy (clipboard paste) ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FsCopyInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub from_rel: String,
    pub to_rel: String,
}

#[tauri::command]
pub async fn fs_copy(input: FsCopyInput, state: State<'_, AppState>) -> Result<(), TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let from = fs_infra::safe_join(&env, &root, &input.from_rel)?;
    let to = fs_infra::safe_join(&env, &root, &input.to_rel)?;
    fs_infra::copy(&env, &state.agent_pool, from, to).await?;
    Ok(())
}

// ────── absolute path (copy path / reference) ──────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FsAbsPathInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
}

/// Resolve a worktree-relative path to its absolute form without touching the
/// disk. For Windows projects this is a native path; for WSL it's the POSIX
/// path inside the distro. Used by the "Copy path" context-menu action.
#[tauri::command]
pub async fn fs_abs_path(
    input: FsAbsPathInput,
    state: State<'_, AppState>,
) -> Result<String, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    Ok(fs_infra::join_inside_worktree(
        &env,
        &root,
        &input.rel_path,
    )?)
}

// ────── reveal in OS file manager ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FsRevealInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
}

/// Open the OS file manager (Windows Explorer) with the target selected.
///
/// This is one of the two sanctioned Windows-side touches of a WSL path
/// (PLAN.md §13): for WSL projects we translate the POSIX path to its
/// `\\wsl.localhost\<distro>\...` UNC form and hand it to Explorer. Hot-path
/// fs ops still route through the agent — only this one-shot UX touches UNC.
#[tauri::command]
pub async fn fs_reveal(
    input: FsRevealInput,
    state: State<'_, AppState>,
) -> Result<(), TauriFsError> {
    use oxyris_core::Environment;
    use oxyris_procutil::HideConsole;
    use std::process::Command;

    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    let windows_path = match &env {
        Environment::Local => abs,
        Environment::Wsl { distro } => crate::infra::path_translator::to_windows(distro, &abs)
            .map_err(|e| TauriFsError::Backend(format!("path translate: {e}")))?,
    };
    // `explorer.exe /select,<path>` highlights the entry in its parent folder.
    // Explorer routinely returns a non-zero exit code even on success, so we
    // intentionally don't inspect the status.
    Command::new("explorer.exe")
        .arg(format!("/select,{windows_path}"))
        .hide_console()
        .spawn()
        .map_err(|e| TauriFsError::Backend(format!("spawn explorer: {e}")))?;
    Ok(())
}

// ────── quick file search (Ctrl+P) ────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FsSearchInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
}

fn default_search_limit() -> u32 {
    50
}

#[derive(Debug, Serialize)]
pub struct FsSearchOutput {
    pub hits: Vec<oxyris_ipc::ops::FsSearchHit>,
    pub truncated: bool,
}

#[tauri::command]
pub async fn fs_search_paths(
    input: FsSearchInput,
    state: State<'_, AppState>,
) -> Result<FsSearchOutput, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let result =
        fs_infra::search_paths(&env, &state.agent_pool, root, input.query, input.limit).await?;
    Ok(FsSearchOutput {
        hits: result.hits,
        truncated: result.truncated,
    })
}

// ────── full-text search (Find in Files / Ctrl+Shift+F) ────────────────────

#[derive(Debug, Deserialize)]
pub struct FsSearchContentInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub include_glob: Option<String>,
    #[serde(default = "default_content_limit")]
    pub max_results: u32,
}

fn default_content_limit() -> u32 {
    1000
}

#[tauri::command]
pub async fn fs_search_content(
    input: FsSearchContentInput,
    state: State<'_, AppState>,
) -> Result<oxyris_ipc::ops::FsSearchContentResult, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let args = oxyris_ipc::ops::FsSearchContentArgs {
        // `root` is filled per-env by the facade; pass empty here.
        root: String::new(),
        query: input.query,
        case_sensitive: input.case_sensitive,
        is_regex: input.is_regex,
        whole_word: input.whole_word,
        include_glob: input.include_glob,
        max_results: input.max_results,
    };
    let result = fs_infra::search_content(&env, &state.agent_pool, root, args).await?;
    Ok(result)
}

// ────── binary read for previews (images, PDFs) ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct FsReadFileBytesInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    /// Cap on bytes; defaults to 16 MiB so images and small PDFs survive,
    /// but a 200 MB binary doesn't blow up the WebView.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct FsReadFileBytesOutput {
    pub abs_path: String,
    /// Base64-encoded file contents. Frontend wraps in `data:<mime>;base64,...`
    /// for `<img>`/`<iframe>` srcs.
    pub bytes_b64: String,
    /// Best-guess MIME from the file extension; `application/octet-stream`
    /// when unknown.
    pub mime: String,
    pub bytes_read: u64,
    pub truncated: bool,
}

const PREVIEW_DEFAULT_CAP: u64 = 16 * 1024 * 1024;

#[tauri::command]
pub async fn fs_read_file_bytes(
    input: FsReadFileBytesInput,
    state: State<'_, AppState>,
) -> Result<FsReadFileBytesOutput, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    // Both branches go through the shared facade now (`fs_infra::read_bytes`):
    // Windows reads via std::fs in spawn_blocking, WSL routes through the
    // per-distro agent's binary-safe `fs.read_bytes` op.
    let bytes_result = fs_infra::read_bytes(
        &env,
        &state.agent_pool,
        abs.clone(),
        Some(input.max_bytes.unwrap_or(PREVIEW_DEFAULT_CAP)),
    )
    .await?;
    let mime = mime_for_path(&input.rel_path);
    Ok(FsReadFileBytesOutput {
        abs_path: abs,
        bytes_b64: bytes_result.bytes_b64,
        mime,
        bytes_read: bytes_result.bytes_read,
        truncated: bytes_result.truncated,
    })
}

fn mime_for_path(p: &str) -> String {
    let ext = p.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_owned()
}

// ────── external editor launcher ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FsOpenExternalInput {
    pub project_id: AggregateId,
    pub worktree_id: AggregateId,
    pub rel_path: String,
    /// Optional explicit editor id ("vscode", "cursor", "sublime",
    /// "notepad++", "default"). When `None`, autodetect order applies.
    #[serde(default)]
    pub editor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FsOpenExternalOutput {
    pub editor: String,
    pub command: String,
}

#[tauri::command]
pub async fn fs_open_external(
    input: FsOpenExternalInput,
    state: State<'_, AppState>,
) -> Result<FsOpenExternalOutput, TauriFsError> {
    let (env, root) = fs_infra::resolve_worktree(&state, input.project_id, input.worktree_id)?;
    let abs = fs_infra::safe_join(&env, &root, &input.rel_path)?;
    let target = open_external::resolve(&env, &abs, input.editor.as_deref())
        .ok_or_else(|| TauriFsError::Backend("no external editor found".into()))?;
    open_external::launch(&target).map_err(|e| TauriFsError::Backend(e.to_string()))?;
    Ok(FsOpenExternalOutput {
        editor: target.editor.into(),
        command: target.display_command,
    })
}

#[derive(Debug, Serialize)]
pub struct ExternalEditorInfo {
    pub id: String,
    pub label: String,
    pub available: bool,
}

#[tauri::command]
pub fn fs_external_editors() -> Vec<ExternalEditorInfo> {
    open_external::detected()
}

mod open_external {
    use super::ExternalEditorInfo;
    use oxyris_core::Environment;
    use std::path::PathBuf;
    use std::process::Command;

    /// Editor entries known to Oxyris, in autodetect priority.
    const KNOWN: &[(&str, &str, &[&str])] = &[
        ("vscode", "Visual Studio Code", &["code.cmd", "code"]),
        ("cursor", "Cursor", &["cursor.cmd", "cursor"]),
        ("windsurf", "Windsurf", &["windsurf.cmd", "windsurf"]),
        ("zed", "Zed", &["zed.exe", "zed"]),
        ("sublime", "Sublime Text", &["subl.exe", "sublime_text.exe"]),
        ("notepad++", "Notepad++", &["notepad++.exe"]),
    ];

    pub struct Resolved {
        pub editor: &'static str,
        pub program: PathBuf,
        pub args: Vec<String>,
        pub display_command: String,
    }

    pub fn detected() -> Vec<ExternalEditorInfo> {
        let mut out = Vec::new();
        for (id, label, bins) in KNOWN {
            let available = bins.iter().any(|b| which::which(b).is_ok());
            out.push(ExternalEditorInfo {
                id: (*id).into(),
                label: (*label).into(),
                available,
            });
        }
        out.push(ExternalEditorInfo {
            id: "default".into(),
            label: "System default".into(),
            available: true,
        });
        out
    }

    /// Pick the editor + build the command for the file. `editor_pref` may
    /// be `None` (autodetect), `Some("default")` (system default via
    /// `ShellExecuteW`), or `Some(id)` (explicit). For WSL projects we
    /// prefer `code --remote wsl+<distro>` when VSCode is the chosen
    /// editor.
    pub fn resolve(
        env: &Environment,
        abs_path: &str,
        editor_pref: Option<&str>,
    ) -> Option<Resolved> {
        if matches!(editor_pref, Some("default")) {
            return Some(default_open(env, abs_path));
        }
        if let Some(id) = editor_pref
            && let Some(r) = build_for(env, abs_path, id)
        {
            return Some(r);
        }
        for (id, _, _) in KNOWN {
            if let Some(r) = build_for(env, abs_path, id) {
                return Some(r);
            }
        }
        Some(default_open(env, abs_path))
    }

    fn build_for(env: &Environment, abs_path: &str, id: &str) -> Option<Resolved> {
        let (_, _, bins) = KNOWN.iter().find(|(eid, _, _)| *eid == id)?;
        let program = bins.iter().find_map(|b| which::which(b).ok())?;

        let (args, display) = match (id, env) {
            // VSCode + Cursor support `--remote wsl+<distro>` so the editor
            // attaches to the distro instead of mounting via UNC.
            ("vscode" | "cursor", Environment::Wsl { distro }) => {
                let args = vec![
                    "--remote".into(),
                    format!("wsl+{distro}"),
                    "--goto".into(),
                    abs_path.to_owned(),
                ];
                let display = format!(
                    "{} --remote wsl+{} --goto {}",
                    program.display(),
                    distro,
                    abs_path
                );
                (args, display)
            }
            _ => {
                let path_for_editor = path_for_editor(env, abs_path);
                let args = vec![path_for_editor.clone()];
                let display = format!("{} {}", program.display(), path_for_editor);
                (args, display)
            }
        };

        Some(Resolved {
            editor: KNOWN.iter().find(|(eid, _, _)| *eid == id)?.0,
            program,
            args,
            display_command: display,
        })
    }

    fn default_open(env: &Environment, abs_path: &str) -> Resolved {
        // `cmd /c start "" <path>` is the cleanest way to invoke the
        // Windows shell-association open for a file from a non-elevated
        // process. The empty `""` is the title arg `start` consumes.
        let path_for_editor = path_for_editor(env, abs_path);
        let display = format!("cmd /c start \"\" {path_for_editor}");
        Resolved {
            editor: "default",
            program: PathBuf::from("cmd"),
            args: vec!["/c".into(), "start".into(), "".into(), path_for_editor],
            display_command: display,
        }
    }

    /// Editors that don't speak WSL natively need the file as a Windows
    /// UNC path. Translate via `wslpath -w` once; fall back to the raw
    /// posix path if translation fails (better than nothing).
    fn path_for_editor(env: &Environment, abs_path: &str) -> String {
        match env {
            Environment::Local => abs_path.to_owned(),
            Environment::Wsl { distro } => {
                crate::infra::path_translator::to_windows(distro, abs_path)
                    .unwrap_or_else(|_| abs_path.to_owned())
            }
        }
    }

    pub fn launch(target: &Resolved) -> std::io::Result<()> {
        use oxyris_procutil::HideConsole;
        Command::new(&target.program)
            .args(&target.args)
            .hide_console()
            .spawn()?;
        Ok(())
    }
}
