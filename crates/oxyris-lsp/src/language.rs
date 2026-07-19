//! Language detection from workspace files + binary discovery for the LSP
//! servers we know about. Adding a new language is a `match` arm here plus
//! tests in the consuming crate.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LspLanguage {
    Rust,
    /// Same server (typescript-language-server) handles both — we don't
    /// distinguish at the LSP layer, just at file extension.
    TypeScriptJavaScript,
    Php,
}

impl LspLanguage {
    pub fn id(self) -> &'static str {
        match self {
            LspLanguage::Rust => "rust",
            LspLanguage::TypeScriptJavaScript => "typescript-javascript",
            LspLanguage::Php => "php",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            LspLanguage::Rust => "rust-analyzer",
            LspLanguage::TypeScriptJavaScript => "typescript-language-server",
            LspLanguage::Php => "intelephense / phpactor",
        }
    }

    /// `initializationOptions` handed to the server on `initialize`. `None`
    /// leaves the server on its defaults.
    ///
    /// rust-analyzer on defaults was the main WSL/host memory sink: a warmed
    /// server sat at 4–5 GB resident, and across several worktrees that
    /// ballooned WSL until the VM froze. We bound that WITHOUT crippling the
    /// `oxyris_lsp_diagnostics` MCP tool — that tool is how the agent verifies
    /// its own edits (trait bounds, lifetimes, macro errors) cheaply after an
    /// edit instead of running a full `cargo build`, and those errors only
    /// come from the `cargo check` layer. So `checkOnSave` stays ON; the
    /// memory cut comes from bounding rust-analyzer's own analysis instead:
    /// - `cachePriming.enable: false` — the biggest resident cut; no eager
    ///   index of every dependency at startup, symbols analysed lazily on
    ///   first query.
    /// - `numThreads: 4` — bound worker parallelism so a spawn burst can't
    ///   saturate every core at once.
    /// - `lru.capacity: 128` — bound the query-analysis cache.
    /// - `check` on a separate `--target-dir` — keeps diagnostics, but the
    ///   on-save `cargo check` no longer shares `target/` with the user's own
    ///   `cargo build`. Sharing it means every save invalidates the build
    ///   cache and every build invalidates check's — a rebuild-thrash that
    ///   doubles CPU/RAM churn. An isolated dir costs extra disk but keeps the
    ///   two from fighting.
    ///
    /// The keys are the `rust-analyzer.*` settings with the `rust-analyzer.`
    /// prefix stripped (that's how they're nested in `initializationOptions`).
    pub fn initialization_options(self) -> Option<serde_json::Value> {
        match self {
            LspLanguage::Rust => Some(serde_json::json!({
                "cachePriming": { "enable": false },
                "numThreads": 4,
                "lru": { "capacity": 128 },
                "check": {
                    "command": "check",
                    "extraArgs": ["--target-dir", "target/rust-analyzer"],
                },
            })),
            // tsserver / intelephense hold far less and have no equivalent
            // build-on-save subprocess — leave them on defaults for now.
            LspLanguage::TypeScriptJavaScript | LspLanguage::Php => None,
        }
    }
}

/// Scan workspace markers and return the detected languages. Order is
/// stable: first hit wins for "primary".
pub fn detect_languages(workspace: &Path) -> Vec<LspLanguage> {
    let mut out: Vec<LspLanguage> = Vec::new();
    if workspace.join("Cargo.toml").is_file() {
        out.push(LspLanguage::Rust);
    }
    if workspace.join("package.json").is_file() || workspace.join("tsconfig.json").is_file() {
        out.push(LspLanguage::TypeScriptJavaScript);
    }
    if workspace.join("composer.json").is_file() || has_php_files(workspace) {
        out.push(LspLanguage::Php);
    }
    out
}

fn has_php_files(workspace: &Path) -> bool {
    // Cheap check — only scan one level deep so we don't burn time on huge
    // monorepos. If the user has a php file in the root, that's enough
    // signal.
    let Ok(entries) = std::fs::read_dir(workspace) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("php"))
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Map a file path to its LSP `languageId` for `didOpen`. Returns `None`
/// for unsupported extensions.
pub fn language_id_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "php" | "phtml" => "php",
        _ => return None,
    })
}

/// Locate the LSP server binary on PATH. Returns the resolved path plus the
/// args we recommend invoking it with. `Err` when no acceptable binary is
/// installed — caller surfaces that to the user as "install rust-analyzer
/// (or whatever) to get this feature".
pub fn resolve_server(lang: LspLanguage) -> Result<(PathBuf, Vec<&'static str>), String> {
    match lang {
        LspLanguage::Rust => which::which("rust-analyzer")
            .map(|p| (p, vec![]))
            .map_err(|_| "rust-analyzer not on PATH; install via rustup component add or release binary".to_owned()),
        LspLanguage::TypeScriptJavaScript => which::which("typescript-language-server")
            .map(|p| (p, vec!["--stdio"]))
            .map_err(|_| "typescript-language-server not on PATH; npm install -g typescript typescript-language-server".to_owned()),
        LspLanguage::Php => {
            // Prefer intelephense (more polished); fall back to phpactor.
            if let Ok(p) = which::which("intelephense") {
                return Ok((p, vec!["--stdio"]));
            }
            if let Ok(p) = which::which("phpactor") {
                return Ok((p, vec!["language-server"]));
            }
            Err("no PHP language server on PATH; install intelephense (npm install -g intelephense) or phpactor".into())
        }
    }
}
