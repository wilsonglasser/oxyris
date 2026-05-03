//! Oxyris MCP server — exposes the per-worktree symbol index over MCP
//! (JSON-RPC 2.0 over stdio) so Claude Code (or other MCP clients) can query
//! it as tools.
//!
//! Spawned by Claude as a child process via the per-session `mcp-config.json`
//! that the supervisor writes. The single required CLI arg is the absolute
//! path to the worktree's `.oxyris/index.db` file (`--index-db`). The
//! `--workspace` arg is required when LSP-backed tools should be enabled —
//! it tells us where to root rust-analyzer / typescript-language-server /
//! intelephense.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxyris_index::Index;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::laravel_state::LaravelState;
use crate::lsp_backend::LspBackend;
use crate::lsp_bridge_client::LspBridgeClient;
use crate::lsp_manager::LspManager;

mod laravel_state;
mod lsp_backend;
mod lsp_bridge_client;
mod lsp_manager;
mod tools;

const PROTOCOL_VERSION: &str = "2024-11-05";

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    install_logging();
    let cli = parse_cli().unwrap_or_else(|e| {
        eprintln!("oxyris-mcp: {e}");
        std::process::exit(2);
    });

    // Open the index lazily so that a missing/empty DB still lets us return
    // empty results rather than crash on initialize.
    let index = match Index::open(Path::new(&cli.index_db)) {
        Ok(i) => Some(Arc::new(i)),
        Err(e) => {
            tracing::warn!(path = %cli.index_db, error = %e, "failed to open index DB; tools will return empty");
            None
        }
    };

    // Pick the LSP backend. Preference order:
    // 1. `--lsp-bridge tcp://…` → proxy to desktop. One shared LSP across
    //    every Claude session in this worktree.
    // 2. `--workspace <dir>` → spawn our own LSPs locally. Fallback for
    //    standalone runs (no Oxyris desktop, or older versions).
    // 3. Neither → LSP tools absent from `tools/list`.
    let lsp: Option<Arc<LspBackend>> = match (&cli.lsp_bridge, &cli.workspace) {
        (Some(addr), Some(workspace)) => {
            tracing::info!(addr = %addr, workspace = %workspace.display(), "lsp: using bridge");
            Some(Arc::new(LspBackend::Bridge {
                client: Arc::new(LspBridgeClient::new(addr, workspace.clone())),
            }))
        }
        (Some(addr), None) => {
            tracing::warn!(addr = %addr, "lsp: --lsp-bridge given without --workspace; ignoring");
            None
        }
        (None, Some(workspace)) => {
            tracing::info!(workspace = %workspace.display(), "lsp: spawning local LSPs (no bridge)");
            Some(Arc::new(LspBackend::Local {
                manager: Arc::new(LspManager::new(workspace.clone())),
            }))
        }
        (None, None) => None,
    };

    // Laravel introspection — lazy load on first tool call. Detection
    // for `tools/list` is cheap (composer.json read only).
    let laravel = Arc::new(LaravelState::new());
    let laravel_workspace = cli.workspace.clone();
    let laravel_advertised = laravel_workspace
        .as_deref()
        .map(LaravelState::looks_like_laravel)
        .unwrap_or(false);

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = handle_message(
            line,
            index.as_deref(),
            lsp.as_ref(),
            &laravel,
            laravel_workspace.as_deref(),
            laravel_advertised,
        )
        .await;
        if let Some(resp) = response {
            let mut bytes = serde_json::to_vec(&resp).unwrap_or_default();
            bytes.push(b'\n');
            stdout.write_all(&bytes).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

struct Cli {
    index_db: String,
    workspace: Option<PathBuf>,
    lsp_bridge: Option<String>,
}

fn parse_cli() -> Result<Cli, String> {
    let mut index_db: Option<String> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut lsp_bridge: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--index-db" => index_db = args.next(),
            "--workspace" => workspace = args.next().map(PathBuf::from),
            "--lsp-bridge" => lsp_bridge = args.next(),
            "--help" | "-h" => {
                println!(
                    "oxyris-mcp — MCP server exposing the Oxyris symbol index + LSP bridge\n\nUsage:\n  oxyris-mcp --index-db <path> [--workspace <dir>] [--lsp-bridge tcp://host:port]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    let index_db = index_db.ok_or("missing required --index-db <path>")?;
    Ok(Cli {
        index_db,
        workspace,
        lsp_bridge,
    })
}

fn install_logging() {
    // Logs go to stderr — stdout is reserved for protocol messages.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("OXYRIS_MCP_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
}

/// Process a single JSON-RPC message line. Returns `None` for notifications
/// (no `id` field) per the spec — those don't get responses.
async fn handle_message(
    line: &str,
    index: Option<&Index>,
    lsp: Option<&Arc<LspBackend>>,
    laravel: &Arc<LaravelState>,
    laravel_workspace: Option<&Path>,
    laravel_advertised: bool,
) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "non-JSON line");
            return None;
        }
    };
    let id = msg.get("id").cloned();
    let needs_response = id.is_some();
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");

    let result = match method {
        "initialize" => {
            // Pre-warm the primary LSP language so the first user query
            // doesn't pay full cold-start cost.
            if let Some(lsp) = lsp {
                lsp.warm_primary();
            }
            Ok(initialize_response())
        }
        "initialized" | "notifications/initialized" => return None,
        "tools/list" => Ok(tools_list_response(lsp.is_some(), laravel_advertised)),
        "tools/call" => {
            handle_tool_call(msg.get("params"), index, lsp, laravel, laravel_workspace).await
        }
        "ping" => Ok(json!({})),
        "" => Err((-32600, "Invalid request: missing method".to_string())),
        other => Err((-32601, format!("Method not found: {other}"))),
    };

    if !needs_response {
        return None;
    }

    Some(match result {
        Ok(value) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": value,
        }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }),
    })
}

fn initialize_response() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "oxyris-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn tools_list_response(has_lsp: bool, has_laravel: bool) -> Value {
    let mut tools = vec![
        json!({
            "name": "oxyris_find_symbol",
            "description": "Locate a code symbol (function, method, class, struct, enum, trait, interface, type, constant, module) by exact name. Faster and more accurate than Grep for code identifiers — prefer this when the user mentions a known symbol. Returns file paths and 1-based line numbers. Falls back to case-insensitive prefix if the exact name isn't found.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Exact symbol name." },
                    "kind": {
                        "type": "string",
                        "enum": ["function", "method", "class", "struct", "enum", "trait", "interface", "type", "constant", "module"],
                        "description": "Optional filter by symbol kind."
                    },
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": 50,
                        "description": "Max results (default 20, capped at 50)."
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "oxyris_list_symbols",
            "description": "List every top-level symbol in a single file (functions, classes, methods, constants, etc.) with their line numbers. Use this to get a structural outline without reading the whole file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path relative to the worktree root, using forward slashes." }
                },
                "required": ["file"]
            }
        }),
        json!({
            "name": "oxyris_project_map",
            "description": "Get a hierarchical summary of the project: top-level directories, file count and symbol count per directory, plus totals. Use this at the start of a task to orient yourself before diving into specific files.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
    ];

    if has_lsp {
        tools.extend([
            json!({
                "name": "oxyris_lsp_find_references",
                "description": "Find every reference to the symbol at a specific position in a file, using the language server (rust-analyzer / typescript-language-server / intelephense). Slower but more semantically accurate than `oxyris_find_symbol` — use when you need actual call sites including ones with renamed imports, generics, etc. Position is 1-based; the tool converts to LSP 0-based internally.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path relative to the workspace root." },
                        "line": { "type": "integer", "minimum": 1, "description": "1-based line number." },
                        "column": { "type": "integer", "minimum": 1, "description": "1-based column number." },
                        "include_declaration": { "type": "boolean", "description": "Include the declaration site in the results (default false)." }
                    },
                    "required": ["file", "line", "column"]
                }
            }),
            json!({
                "name": "oxyris_lsp_hover",
                "description": "Get the type signature, doc comment, and inferred type for the symbol at a position — what an IDE shows when you hover. Use when you need to understand what a name actually refers to before editing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path relative to the workspace root." },
                        "line": { "type": "integer", "minimum": 1 },
                        "column": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["file", "line", "column"]
                }
            }),
            json!({
                "name": "oxyris_lsp_diagnostics",
                "description": "Return the language server's current diagnostics (compiler errors, warnings, lints) for a file — what an IDE shows in the Problems panel. Empty when no issues are open. Use after editing to verify your changes parse and type-check.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path relative to the workspace root." }
                    },
                    "required": ["file"]
                }
            }),
        ]);
    }

    if has_laravel {
        tools.extend([
            json!({
                "name": "oxyris_laravel_routes",
                "description": "List Laravel routes (HTTP method, URI, controller@action, optional name) discovered statically in `routes/*.php`. Optional `name` argument fuzzy-matches against route names and URIs. Use to find call sites for `route('users.index')` or to verify the URI of a known route name.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Optional substring filter against route name or URI." }
                    }
                }
            }),
            json!({
                "name": "oxyris_laravel_configs",
                "description": "List top-level config keys parsed from `config/*.php`. Each key is reported as `<file>.<key>` (e.g., `app.name`). Optional `prefix` filters by leading dot path. Use to verify keys for `config('database.connections.mysql')`-style lookups.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prefix": { "type": "string", "description": "Optional dot-prefix filter (e.g., `database.`)." }
                    }
                }
            }),
            json!({
                "name": "oxyris_laravel_models",
                "description": "List Eloquent models from `app/Models/**/*.php` with their table name, fillable fields, and detected relations (hasOne/hasMany/belongsTo/etc.). Optional `name` substring filter on the class name. Use before editing model code to understand schema and relationships.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Optional substring filter against model class name." }
                    }
                }
            }),
            json!({
                "name": "oxyris_laravel_blade_components",
                "description": "List Blade views from `resources/views/**/*.blade.php` with their dot-notation names (`admin.users.index`). Optional `name` substring filter. Use to verify view names referenced in `view('...')` calls.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Optional substring filter against view dot-notation." }
                    }
                }
            }),
            json!({
                "name": "oxyris_laravel_observers",
                "description": "List Eloquent observers from `app/Observers/**/*.php` with the model they observe (inferred from class suffix) and the lifecycle events they implement (created/updated/deleting/...). Optional `name` substring filter.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Optional substring filter against observer class name." }
                    }
                }
            }),
            json!({
                "name": "oxyris_laravel_policies",
                "description": "List authorization policies from `app/Policies/**/*.php` with the model authorized (inferred from class suffix) and the abilities (method names) declared. Optional `name` substring filter.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Optional substring filter against policy class name." }
                    }
                }
            }),
            json!({
                "name": "oxyris_laravel_jobs",
                "description": "List queued/dispatchable jobs from `app/Jobs/**/*.php`. Each entry shows whether the class implements `ShouldQueue` and any static `$queue` value. Optional `name` substring filter.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Optional substring filter against job class name." }
                    }
                }
            }),
        ]);
    }

    json!({ "tools": tools })
}

async fn handle_tool_call(
    params: Option<&Value>,
    index: Option<&Index>,
    lsp: Option<&Arc<LspBackend>>,
    laravel: &Arc<LaravelState>,
    laravel_workspace: Option<&Path>,
) -> Result<Value, (i64, String)> {
    let params = params.ok_or((-32602, "missing params".to_string()))?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or((-32602, "missing tool name".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let text = match name {
        "oxyris_find_symbol" => tools::find_symbol(index, &args),
        "oxyris_list_symbols" => tools::list_symbols(index, &args),
        "oxyris_project_map" => tools::project_map(index, &args),
        "oxyris_lsp_find_references" => match lsp {
            Some(lsp) => tools::lsp_find_references(lsp, &args).await,
            None => {
                Err("LSP support not enabled (workspace path not provided to oxyris-mcp)".into())
            }
        },
        "oxyris_lsp_hover" => match lsp {
            Some(lsp) => tools::lsp_hover(lsp, &args).await,
            None => Err("LSP support not enabled".into()),
        },
        "oxyris_lsp_diagnostics" => match lsp {
            Some(lsp) => tools::lsp_diagnostics(lsp, &args).await,
            None => Err("LSP support not enabled".into()),
        },
        "oxyris_laravel_routes" => match laravel_workspace {
            Some(ws) => tools::laravel_routes(laravel, ws, &args).await,
            None => Err("Laravel tools not enabled (--workspace not provided)".into()),
        },
        "oxyris_laravel_configs" => match laravel_workspace {
            Some(ws) => tools::laravel_configs(laravel, ws, &args).await,
            None => Err("Laravel tools not enabled".into()),
        },
        "oxyris_laravel_models" => match laravel_workspace {
            Some(ws) => tools::laravel_models(laravel, ws, &args).await,
            None => Err("Laravel tools not enabled".into()),
        },
        "oxyris_laravel_blade_components" => match laravel_workspace {
            Some(ws) => tools::laravel_blade_components(laravel, ws, &args).await,
            None => Err("Laravel tools not enabled".into()),
        },
        "oxyris_laravel_observers" => match laravel_workspace {
            Some(ws) => tools::laravel_observers(laravel, ws, &args).await,
            None => Err("Laravel tools not enabled".into()),
        },
        "oxyris_laravel_policies" => match laravel_workspace {
            Some(ws) => tools::laravel_policies(laravel, ws, &args).await,
            None => Err("Laravel tools not enabled".into()),
        },
        "oxyris_laravel_jobs" => match laravel_workspace {
            Some(ws) => tools::laravel_jobs(laravel, ws, &args).await,
            None => Err("Laravel tools not enabled".into()),
        },
        other => return Err((-32601, format!("unknown tool: {other}"))),
    }
    .map_err(|e| (-32603, e))?;

    Ok(json!({
        "content": [
            { "type": "text", "text": text }
        ]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn req(line: &str) -> Option<Value> {
        let laravel = Arc::new(LaravelState::new());
        handle_message(line, None, None, &laravel, None, false).await
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let resp = req(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await
            .unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], "oxyris-mcp");
    }

    #[tokio::test]
    async fn notifications_get_no_response() {
        // No id → notification per JSON-RPC 2.0.
        assert!(
            req(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn tools_list_advertises_three_tools_without_lsp() {
        let resp = req(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"oxyris_find_symbol"));
        assert!(names.contains(&"oxyris_list_symbols"));
        assert!(names.contains(&"oxyris_project_map"));
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let resp = req(r#"{"jsonrpc":"2.0","id":9,"method":"explode"}"#)
            .await
            .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn tools_call_without_index_returns_empty_text() {
        let resp = req(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"oxyris_project_map","arguments":{}}}"#,
        )
        .await
        .unwrap();
        assert!(resp["result"]["content"][0]["text"].as_str().is_some());
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_returns_error() {
        let resp = req(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"bogus","arguments":{}}}"#,
        )
        .await
        .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn lsp_tool_without_workspace_returns_helpful_error() {
        let resp = req(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"oxyris_lsp_find_references","arguments":{"file":"x.rs","line":1,"column":1}}}"#,
        )
        .await
        .unwrap();
        assert_eq!(resp["error"]["code"], -32603);
        let msg = resp["error"]["message"].as_str().unwrap_or("");
        assert!(msg.contains("LSP"), "got: {msg}");
    }
}
