//! Build the per-worktree MCP config that we hand to Claude via
//! `--mcp-config <path>`. Spawning the MCP server itself is Claude's job —
//! we just produce the config file pointing at the right index DB and the
//! right binary.

use std::path::{Path, PathBuf};

use oxyris_core::Environment;
use serde_json::json;

use crate::infra::path_translator;

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
///
/// When `session` is `Some`, the config is written per-session as
/// `mcp-<session>.json` (instead of the shared `mcp.json`) and carries
/// `--session-id`. Combined with `Some(autopilot_bridge_port)` it also wires the
/// `oxyris_autopilot_*` tools so the pure-mode claude can hand the wheel to the
/// pilot for its own session. Structured-provider callers pass `None` for both
/// and keep the shared, autopilot-less config.
pub fn prepare_for_worktree(
    env: &Environment,
    worktree_root: &str,
    lsp_bridge_port: Option<u16>,
    session: Option<&str>,
    autopilot_bridge_port: Option<u16>,
    browser_bridge_port: Option<u16>,
) -> std::io::Result<Option<McpSetup>> {
    let Some(bin) = resolve_mcp_bin() else {
        tracing::debug!("oxyris-mcp binary not located; skipping MCP config");
        return Ok(None);
    };

    // For WSL projects, the LSP bridge needs the POSIX path inside the
    // distro (rust-analyzer running in Linux can't read `\\wsl.localhost`).
    // The index DB stays Windows-side at `<worktree>/.oxyris/index.db`
    // because SQLite over 9p is slow/crash-prone — we mount via the
    // Windows UNC for that one file.
    let workspace_for_args = match env {
        Environment::Local => worktree_root.to_owned(),
        Environment::Wsl { distro } => match path_translator::to_posix(distro, worktree_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, distro, worktree = worktree_root, "mcp: wslpath translation failed; skipping MCP config");
                return Ok(None);
            }
        },
    };

    let oxyris_dir = Path::new(worktree_root).join(".oxyris");
    std::fs::create_dir_all(&oxyris_dir)?;
    let index_db = oxyris_dir.join("index.db");
    // Per-session config when a session id is given (so each carries its own
    // `--session-id` for the autopilot tools); the shared `mcp.json` otherwise.
    let config_path = match session {
        Some(id) => oxyris_dir.join(format!("mcp-{id}.json")),
        None => oxyris_dir.join("mcp.json"),
    };

    // Build args. `--lsp-bridge` is included only when the desktop's TCP
    // bridge is actually up — fallback path lets the MCP server spawn its
    // own LSPs (the pre-bridge behaviour) instead of erroring out.
    let mut args: Vec<String> = vec![
        "--index-db".into(),
        index_db.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_for_args,
    ];
    if let Some(port) = lsp_bridge_port {
        args.push("--lsp-bridge".into());
        args.push(format!("tcp://127.0.0.1:{port}"));
    }
    // Autopilot hand-off tools: only wired when we have both the calling
    // session's id and a live control bridge. The session id is baked here
    // (never a tool argument) so Claude can only ever engage its own session.
    let autopilot_wired = match (session, autopilot_bridge_port) {
        (Some(id), Some(port)) => {
            args.push("--session-id".into());
            args.push(id.to_owned());
            args.push("--autopilot-bridge".into());
            args.push(format!("tcp://127.0.0.1:{port}"));
            true
        }
        _ => false,
    };
    // Browser tools: shared headless browser, not session-scoped, so any
    // session gets them when the bridge is up.
    let browser_wired = if let Some(port) = browser_bridge_port {
        args.push("--browser-bridge".into());
        args.push(format!("tcp://127.0.0.1:{port}"));
        true
    } else {
        false
    };

    let contents = json!({
        "mcpServers": {
            // Capitalized so Claude renders chips as `Oxyris > lsp_hover`,
            // unambiguously distinct from any other LSP MCP the user might
            // have registered globally.
            "Oxyris": {
                "command": bin.to_string_lossy(),
                "args": args,
            }
        }
    });
    std::fs::write(&config_path, serde_json::to_vec_pretty(&contents)?)?;

    let mut system_prompt_nudge = SYSTEM_PROMPT_NUDGE.to_owned();
    if autopilot_wired {
        system_prompt_nudge.push_str("\n\n");
        system_prompt_nudge.push_str(AUTOPILOT_NUDGE);
    }
    if browser_wired {
        system_prompt_nudge.push_str("\n\n");
        system_prompt_nudge.push_str(BROWSER_NUDGE);
    }

    Ok(Some(McpSetup {
        config_path: config_path.to_string_lossy().into_owned(),
        system_prompt_nudge,
    }))
}

/// Appended to the system prompt so Claude prefers the MCP tools over Grep
/// when looking for code symbols. Kept short — tool descriptions carry the
/// detailed contract.
/// Appended only when the autopilot hand-off tools are wired (pure-mode session
/// with a live control bridge). Tells Claude the tools exist and that engaging
/// is a one-shot hand-off, not a conversation.
const AUTOPILOT_NUDGE: &str = r#"This session can hand itself off to the Oxyris auto-pilot — a supervisor agent that drives this same Claude session autonomously toward a stated mission:
- `oxyris_autopilot_engage(mission)` — turn the pilot ON for THIS session with a mission (a concrete spec of what to accomplish), then STOP and end your turn. The pilot takes over from there; do not keep calling it or talking to it. Use when the user asks you to "let the autopilot finish/continue this" or to run long autonomous work unattended.
- `oxyris_autopilot_disengage()` — turn the pilot OFF for this session.
The supervisor backend/model comes from the user's saved Settings — you only supply the mission. Engage is fire-and-forget: call it once, then stop."#;

/// Appended when the browser bridge is wired. Tells Claude the headless-browser
/// tools exist so it can navigate + screenshot to validate its own work.
const BROWSER_NUDGE: &str = r#"This session has a shared headless browser for validating work in a real page (open a dev server, check a UI, read rendered output):
- `browser_navigate(url)` — go to a URL (e.g. http://localhost:5173) and wait for load.
- `browser_screenshot()` — capture the current page as a PNG image you can look at to verify it renders correctly.
- `browser_snapshot()` — the page's visible text, when you don't need pixels.
- `browser_click(selector)` / `browser_type(selector, text)` — interact via CSS selectors.
- `browser_eval(expression)` — run JavaScript in the page and get the result.
- `browser_wait_for(selector)` — wait until an element appears.
Prefer screenshotting to confirm a frontend change actually looks right instead of assuming. The browser launches on first use."#;

const SYSTEM_PROMPT_NUDGE: &str = r#"This project ships an `Oxyris` MCP server with a tree-sitter symbol index, an LSP bridge, and (when the workspace is a Laravel app) Laravel-aware tools.

For code identifiers (function/class/struct/type names), prefer `oxyris_find_symbol` over Grep — it returns precise file:line locations across all indexed languages and falls back to a case-insensitive prefix when the exact name isn't found. Use `oxyris_list_symbols` for a file outline before reading large files end-to-end, and `oxyris_project_map` at the start of unfamiliar tasks.

When you need semantic accuracy (find every call site including renamed imports / generics / through dynamic dispatch, get an inferred type, or check compiler errors), use the LSP-backed tools:
- `oxyris_lsp_find_references(file, line, column)` — every reference, computed by rust-analyzer / typescript-language-server / intelephense.
- `oxyris_lsp_hover(file, line, column)` — type signature and doc comment at a position.
- `oxyris_lsp_diagnostics(file)` — compiler errors and lints currently open in the file. Run after edits to verify.

When the workspace is a Laravel project (composer.json with `laravel/framework`), the following tools become available — prefer them over Grep when reasoning about route names, config keys, Eloquent models, or Blade views:
- `oxyris_laravel_routes(name?)` — list HTTP method/URI/controller@action/name plus `{middleware}` chip pulled from `routes/*.php`. Resource/apiResource calls expand into the conventional 7/5 endpoints, and `Route::group(['prefix'=>...,'middleware'=>...], fn)` (array syntax) plus `Route::prefix(...)->group(fn)` (chained) propagate prefixes/middleware to nested routes. Use to verify what `route('...')` resolves to, find a handler, or check what middleware guards a URI.
- `oxyris_laravel_configs(prefix?)` — top-level keys from `config/*.php`. Use before writing `config('foo.bar')` to confirm the key exists.
- `oxyris_laravel_models(name?)` — Eloquent class + table + fillable + relations from `app/Models/**`. Use before editing or querying a model so you know its schema and relationship names.
- `oxyris_laravel_blade_components(name?)` — view dot-notation map. Use to verify what `view('...')` references exist.
- `oxyris_laravel_observers(name?)` — Eloquent observers from `app/Observers/**` with model + lifecycle hooks (created/updated/deleting/...).
- `oxyris_laravel_policies(name?)` — authorization policies from `app/Policies/**` with model + abilities (viewAny/view/update/delete/...).
- `oxyris_laravel_jobs(name?)` — queueable/sync jobs from `app/Jobs/**` with `ShouldQueue` flag and any static `$queue` value.

All layers are valid: tree-sitter is fast/structural, LSP is slow/precise, Laravel tools cover framework-specific magic strings. Use whichever fits."#;
