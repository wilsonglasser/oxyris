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

use crate::lsp_bridge_client::LspBridgeClient;
use crate::lsp_manager::LspManager;

pub enum LspBackend {
    Local { manager: Arc<LspManager> },
    Bridge { client: Arc<LspBridgeClient> },
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

    pub async fn diagnostics(&self, file: &Path) -> Result<Vec<Diagnostic>, String> {
        match self {
            LspBackend::Local { manager } => {
                let lang = manager
                    .language_for(file)
                    .ok_or_else(|| format!("no LSP server is enabled for {}", file.display()))?;
                let client = manager.get(lang).await?;
                client
                    .ensure_open(file)
                    .await
                    .map_err(|e| format!("lsp: {e}"))?;
                client
                    .diagnostics_for(file)
                    .await
                    .map_err(|e| format!("lsp: {e}"))
            }
            LspBackend::Bridge { client } => client.diagnostics(file).await,
        }
    }

    pub fn resolve_path(&self, file: &str) -> PathBuf {
        let path = PathBuf::from(file);
        if path.is_absolute() {
            return path;
        }
        self.workspace().join(path)
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
