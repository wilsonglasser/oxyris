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
