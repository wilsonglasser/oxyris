//! Build the per-worktree MCP config that we hand to Claude via
//! `--mcp-config <path>`. Spawning the MCP server itself is Claude's job —
//! we just produce the config file pointing at the right index DB and the
//! right binary.

use std::path::{Path, PathBuf};

use oxyris_core::Environment;
use serde_json::json;

/// What we plant on disk so Claude can spawn the Oxyris MCP server.
pub struct McpSetup {
    /// Absolute path to the JSON config (`--mcp-config <this>`).
    pub config_path: String,
    /// System-prompt nudge — append via `--append-system-prompt` so Claude
    /// knows the tools exist and prefers them over Grep.
    pub system_prompt_nudge: String,
}

/// Resolve the path to the `oxyris-mcp` binary the same way
/// `AgentPool::resolve_host_agent_path` resolves the agent: explicit env
/// override first, then a dev-mode walk from the current exe, then the
/// production default sitting next to `oxyris-desktop.exe`.
pub fn resolve_mcp_bin() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("OXYRIS_MCP_BIN_PATH") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let candidate = exe_dir.join(mcp_bin_filename());
    if candidate.exists() {
        return Some(candidate);
    }
    // Dev flow: cargo build puts both binaries side-by-side under target/<profile>.
    // current_exe for `cargo tauri dev` is the desktop binary; sibling name
    // covers it.
    None
}

#[cfg(target_os = "windows")]
fn mcp_bin_filename() -> &'static str {
    "oxyris-mcp.exe"
}

#[cfg(not(target_os = "windows"))]
fn mcp_bin_filename() -> &'static str {
    "oxyris-mcp"
}

/// Generate the MCP config JSON for a worktree and write it next to its
/// index DB at `<worktree>/.oxyris/mcp.json`. Returns the absolute path so
/// the caller can pass it through `--mcp-config`.
///
/// Returns `None` when:
/// - the project is not Windows (WSL MCP setup lands in the next sprint), or
/// - the MCP binary can't be located on disk (don't generate a config that
///   would point at a non-existent command).
pub fn prepare_for_worktree(
    env: &Environment,
    worktree_root: &str,
) -> std::io::Result<Option<McpSetup>> {
    if !matches!(env, Environment::Windows) {
        return Ok(None);
    }
    let Some(bin) = resolve_mcp_bin() else {
        tracing::debug!("oxyris-mcp binary not located; skipping MCP config");
        return Ok(None);
    };

    let oxyris_dir = Path::new(worktree_root).join(".oxyris");
    std::fs::create_dir_all(&oxyris_dir)?;
    let index_db = oxyris_dir.join("index.db");
    let config_path = oxyris_dir.join("mcp.json");

    let contents = json!({
        "mcpServers": {
            "oxyris": {
                "command": bin.to_string_lossy(),
                "args": ["--index-db", index_db.to_string_lossy()]
            }
        }
    });
    std::fs::write(&config_path, serde_json::to_vec_pretty(&contents)?)?;

    Ok(Some(McpSetup {
        config_path: config_path.to_string_lossy().into_owned(),
        system_prompt_nudge: SYSTEM_PROMPT_NUDGE.to_owned(),
    }))
}

/// Appended to the system prompt so Claude prefers the MCP tools over Grep
/// when looking for code symbols. Kept short — tool descriptions carry the
/// detailed contract.
const SYSTEM_PROMPT_NUDGE: &str = r#"This project ships an `oxyris` MCP server with a pre-built tree-sitter symbol index. \
Prefer `oxyris_find_symbol` over Grep when the user mentions a code identifier (function, class, struct, type, etc.) — \
it returns precise file:line locations across all indexed languages and falls back to a case-insensitive prefix \
when the exact name isn't found. Use `oxyris_list_symbols` for a file outline before reading large files end-to-end, \
and `oxyris_project_map` at the start of unfamiliar tasks to orient yourself."#;
