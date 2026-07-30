//! Thin enum that lets tools target either:
//! - a `Local` `LspManager` that spawns its own LSPs (fallback when no
//!   bridge URL is supplied — keeps things working for users on older
//!   Oxyris versions or when running `oxyris-mcp` standalone), or
//! - a `Bridge` TCP client proxying every call to the desktop's shared
//!   `LspManager` (preferred — eliminates the rust-analyzer-per-session
//!   duplication).
//!
//! Same async surface from both arms; tool code doesn't need to care.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxyris_lsp::lsp_types::{Diagnostic, Location};
use oxyris_lsp::{LspClient, LspLanguage};
use serde::{Deserialize, Serialize};

use crate::lsp_bridge_client::LspBridgeClient;
use crate::lsp_manager::LspManager;

pub enum LspBackend {
    Local { manager: Arc<LspManager> },
    Bridge { client: Arc<LspBridgeClient> },
}

/// Diagnostics the server published against one document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiagnostics {
    /// Document URI as the server reported it. Render with
    /// [`LspBackend::uri_to_display`].
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Result of [`LspBackend::check`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckReport {
    pub files: Vec<FileDiagnostics>,
    /// True when a `cargo check` actually ran to completion for this report.
    /// False means the numbers come from the server's own analysis only —
    /// either the language has no check layer, or the check timed out.
    pub checked: bool,
}

/// Trigger the check layer and wait for it. Rust only: `runFlycheck` is a
/// rust-analyzer extension, and waiting on a server that will never start one
/// would just burn the grace period on every call.
async fn run_check(client: &Arc<LspClient>, file: Option<&Path>, is_rust: bool) -> bool {
    if !is_rust {
        return false;
    }
    match client.run_check_and_wait(file).await {
        Ok(ran) => ran,
        Err(e) => {
            tracing::debug!(error = %e, "lsp: flycheck did not settle");
            false
        }
    }
}

async fn collect(
    client: &Arc<LspClient>,
    file: Option<&Path>,
) -> Result<Vec<FileDiagnostics>, String> {
    match file {
        Some(f) => {
            let diagnostics = client
                .diagnostics_for(f)
                .await
                .map_err(|e| format!("lsp: {e}"))?;
            Ok(vec![FileDiagnostics {
                uri: f.to_string_lossy().into_owned(),
                diagnostics,
            }])
        }
        None => Ok(client
            .all_diagnostics()
            .await
            .into_iter()
            .map(|(uri, diagnostics)| FileDiagnostics {
                uri: uri.to_string(),
                diagnostics,
            })
            .collect()),
    }
}

impl LspBackend {
    pub fn workspace(&self) -> &Path {
        match self {
            LspBackend::Local { manager } => manager.workspace(),
            LspBackend::Bridge { client } => client.workspace(),
        }
    }

    /// Local: kick off background warm of the primary language. Bridge:
    /// no-op — the desktop already warms at worktree create, so any work
    /// here would just race the same spawn over TCP for nothing.
    pub fn warm_primary(self: &Arc<Self>) {
        if let LspBackend::Local { manager } = self.as_ref() {
            manager.warm_primary();
        }
    }

    pub async fn find_references(
        &self,
        file: &Path,
        line: u32,
        column: u32,
        include_declaration: bool,
    ) -> Result<Vec<Location>, String> {
        match self {
            LspBackend::Local { manager } => {
                let lang = manager.language_for(file).ok_or_else(|| {
                    format!(
                        "no LSP server is enabled for {} (extension/language not detected in this workspace)",
                        file.display()
                    )
                })?;
                let client = manager.get(lang).await?;
                client
                    .find_references(file, line, column, include_declaration)
                    .await
                    .map_err(|e| format!("lsp: {e}"))
            }
            LspBackend::Bridge { client } => {
                client
                    .find_references(file, line, column, include_declaration)
                    .await
            }
        }
    }

    pub async fn hover(
        &self,
        file: &Path,
        line: u32,
        column: u32,
    ) -> Result<Option<String>, String> {
        match self {
            LspBackend::Local { manager } => {
                let lang = manager
                    .language_for(file)
                    .ok_or_else(|| format!("no LSP server is enabled for {}", file.display()))?;
                let client = manager.get(lang).await?;
                client
                    .hover(file, line, column)
                    .await
                    .map_err(|e| format!("lsp: {e}"))
            }
            LspBackend::Bridge { client } => client.hover(file, line, column).await,
        }
    }

    /// Run the check layer and return everything it reported.
    ///
    /// `file: Some` scopes the report to one file; `None` covers the whole
    /// workspace. Either way the sequence is: reconcile open documents with
    /// disk (our agent edits files behind the server's back), trigger
    /// `cargo check`, wait for it to finish, then read. That ordering is what
    /// makes this a substitute for the agent shelling out to `cargo check`
    /// itself instead of a stale cache read.
    pub async fn check(&self, file: Option<&Path>) -> Result<CheckReport, String> {
        match self {
            LspBackend::Local { manager } => {
                let lang = match file {
                    Some(f) => manager
                        .language_for(f)
                        .ok_or_else(|| format!("no LSP server is enabled for {}", f.display()))?,
                    None => *manager.detected().first().ok_or_else(|| {
                        "no supported language detected in this workspace".to_string()
                    })?,
                };
                let client = manager.get(lang).await?;
                if let Some(f) = file {
                    client
                        .sync_from_disk(f)
                        .await
                        .map_err(|e| format!("lsp: {e}"))?;
                } else {
                    client.sync_open_from_disk().await;
                }
                let checked = run_check(&client, file, lang == LspLanguage::Rust).await;
                let files = collect(&client, file).await?;
                Ok(CheckReport { files, checked })
            }
            LspBackend::Bridge { client } => client.check(file).await,
        }
    }

    /// Resolve a tool-supplied `file` argument to an absolute path **confined to
    /// the workspace**. The MCP server is driven by a `claude` child running
    /// under `bypassPermissions`, so an unconfined `file` would let it read any
    /// file the process can. Rejects absolute paths and `..`, then canonicalizes
    /// (resolving symlinks) and confirms the real path is still under the
    /// workspace root — closing the symlink-escape hole too.
    pub fn resolve_path(&self, file: &str) -> Result<PathBuf, String> {
        let raw = Path::new(file);
        if raw.is_absolute()
            || raw
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!("path must be relative to the workspace: {file}"));
        }
        let root = self
            .workspace()
            .canonicalize()
            .map_err(|e| format!("workspace unavailable: {e}"))?;
        let real = self
            .workspace()
            .join(raw)
            .canonicalize()
            .map_err(|_| format!("no such file in workspace: {file}"))?;
        if !real.starts_with(&root) {
            return Err(format!("path escapes the workspace: {file}"));
        }
        Ok(real)
    }

    pub fn uri_to_display(&self, uri: &str) -> String {
        let raw = if let Some(rest) = uri.strip_prefix("file:///") {
            rest.to_owned()
        } else if let Some(rest) = uri.strip_prefix("file://") {
            rest.to_owned()
        } else {
            return uri.to_owned();
        };
        let abs = PathBuf::from(&raw);
        if let Ok(rel) = abs.strip_prefix(self.workspace()) {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            raw.replace('\\', "/")
        }
    }
}
