//! Lazy pool of LSP clients per workspace. Each language gets one client,
//! spawned on first use (or by the pre-warm) and reused across tool calls.
//!
//! "Pre-warm" sends a `did_open` for one or two representative files so the
//! server starts indexing while Claude is still composing its first
//! request — by the time the user asks `find_references`, results land
//! quickly instead of cold-starting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxyris_lsp::{LspClient, LspLanguage, detect_languages, resolve_server};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LspState {
    pub language: LspLanguage,
    pub status: LspStatus,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum LspStatus {
    NotInstalled(String),
    Spawning,
    Ready,
    Failed(String),
}

pub struct LspManager {
    workspace: PathBuf,
    detected: Vec<LspLanguage>,
    clients: Mutex<HashMap<LspLanguage, Arc<LspClient>>>,
    states: Mutex<HashMap<LspLanguage, LspStatus>>,
}

impl LspManager {
    pub fn new(workspace: PathBuf) -> Self {
        let detected = detect_languages(&workspace);
        Self {
            workspace,
            detected,
            clients: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    #[allow(dead_code)]
    pub fn detected(&self) -> &[LspLanguage] {
        &self.detected
    }

    /// Resolve which language a file belongs to *and* whose LSP is detected
    /// in this workspace. Returns `None` for unsupported extensions or
    /// languages we haven't detected here (so we don't spawn rust-analyzer
    /// in a pure JS project just because someone Read'd a `.rs` file).
    pub fn language_for(&self, path: &Path) -> Option<LspLanguage> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        let lang = match ext.as_str() {
            "rs" => LspLanguage::Rust,
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => LspLanguage::TypeScriptJavaScript,
            "php" | "phtml" => LspLanguage::Php,
            _ => return None,
        };
        if self.detected.contains(&lang) {
            Some(lang)
        } else {
            None
        }
    }

    /// Get or lazily spawn the client for `lang`. Concurrent calls share
    /// the same in-flight spawn through the mutex.
    pub async fn get(&self, lang: LspLanguage) -> Result<Arc<LspClient>, String> {
        // Fast path — already up.
        {
            let clients = self.clients.lock().await;
            if let Some(c) = clients.get(&lang) {
                return Ok(c.clone());
            }
        }

        // Resolve the binary up-front so we can give a clear error.
        let (binary, args) = resolve_server(lang).inspect_err(|e| {
            self.set_status_blocking(lang, LspStatus::NotInstalled(e.clone()));
        })?;

        // Take the lock, double-check, then spawn.
        let mut clients = self.clients.lock().await;
        if let Some(c) = clients.get(&lang) {
            return Ok(c.clone());
        }
        self.set_status(lang, LspStatus::Spawning).await;
        match LspClient::spawn(
            &binary,
            &args,
            &self.workspace,
            lang.initialization_options(),
        )
        .await
        {
            Ok(client) => {
                clients.insert(lang, client.clone());
                self.set_status(lang, LspStatus::Ready).await;
                Ok(client)
            }
            Err(e) => {
                let msg = e.to_string();
                self.set_status(lang, LspStatus::Failed(msg.clone())).await;
                Err(msg)
            }
        }
    }

    /// Fire-and-forget spawn of the primary language so Claude's first LSP
    /// query doesn't pay the full cold-start. Called once on `initialize`.
    pub fn warm_primary(self: &Arc<Self>) {
        let Some(&primary) = self.detected.first() else {
            return;
        };
        let me = self.clone();
        tokio::spawn(async move {
            match me.get(primary).await {
                Ok(_) => {
                    tracing::info!(?primary, "lsp pre-warm ready");
                }
                Err(e) => {
                    tracing::debug!(?primary, error = %e, "lsp pre-warm skipped");
                }
            }
        });
    }

    #[allow(dead_code)]
    pub async fn statuses(&self) -> Vec<LspState> {
        let states = self.states.lock().await;
        self.detected
            .iter()
            .map(|&lang| LspState {
                language: lang,
                status: states
                    .get(&lang)
                    .cloned()
                    .unwrap_or(LspStatus::NotInstalled(
                        "not yet attempted; will spawn on first use".into(),
                    )),
            })
            .collect()
    }

    async fn set_status(&self, lang: LspLanguage, status: LspStatus) {
        self.states.lock().await.insert(lang, status);
    }

    fn set_status_blocking(&self, lang: LspLanguage, status: LspStatus) {
        // Best-effort sync update from a sync context. If contended we just
        // skip — the status is observability, not correctness.
        if let Ok(mut guard) = self.states.try_lock() {
            guard.insert(lang, status);
        }
    }
}
