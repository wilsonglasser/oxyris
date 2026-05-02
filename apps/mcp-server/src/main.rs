//! Oxyris MCP server — exposes the per-worktree symbol index over MCP
//! (JSON-RPC 2.0 over stdio) so Claude Code (or other MCP clients) can query
//! it as tools.
//!
//! Spawned by Claude as a child process via the per-session `mcp-config.json`
//! that the supervisor writes. The single required CLI arg is the absolute
//! path to the worktree's `.oxyris/index.db` file (`--index-db`).

use std::path::Path;
use std::sync::Arc;

use oxyris_index::Index;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = handle_message(line, index.as_deref());
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
}

fn parse_cli() -> Result<Cli, String> {
    let mut index_db: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--index-db" => index_db = args.next(),
            "--help" | "-h" => {
                println!(
                    "oxyris-mcp — MCP server exposing the Oxyris symbol index\n\nUsage:\n  oxyris-mcp --index-db <path>"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    let index_db = index_db.ok_or("missing required --index-db <path>")?;
    Ok(Cli { index_db })
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
fn handle_message(line: &str, index: Option<&Index>) -> Option<Value> {
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
        "initialize" => Ok(initialize_response()),
        "initialized" | "notifications/initialized" => return None,
        "tools/list" => Ok(tools_list_response()),
        "tools/call" => handle_tool_call(msg.get("params"), index),
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

fn tools_list_response() -> Value {
    json!({
        "tools": [
            {
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
            },
            {
                "name": "oxyris_list_symbols",
                "description": "List every top-level symbol in a single file (functions, classes, methods, constants, etc.) with their line numbers. Use this to get a structural outline without reading the whole file.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path relative to the worktree root, using forward slashes." }
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "oxyris_project_map",
                "description": "Get a hierarchical summary of the project: top-level directories, file count and symbol count per directory, plus totals. Use this at the start of a task to orient yourself before diving into specific files.",
                "inputSchema": { "type": "object", "properties": {} }
            }
        ]
    })
}

fn handle_tool_call(params: Option<&Value>, index: Option<&Index>) -> Result<Value, (i64, String)> {
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

    fn req(line: &str) -> Option<Value> {
        handle_message(line, None)
    }

    #[test]
    fn initialize_returns_server_info() {
        let resp = req(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], "oxyris-mcp");
    }

    #[test]
    fn notifications_get_no_response() {
        // No id → notification per JSON-RPC 2.0.
        assert!(req(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn tools_list_advertises_three_tools() {
        let resp = req(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"oxyris_find_symbol"));
        assert!(names.contains(&"oxyris_list_symbols"));
        assert!(names.contains(&"oxyris_project_map"));
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let resp = req(r#"{"jsonrpc":"2.0","id":9,"method":"explode"}"#).unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn tools_call_without_index_returns_empty_text() {
        // No DB available — tool functions still need to gracefully return
        // something rather than panic.
        let resp = req(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"oxyris_project_map","arguments":{}}}"#,
        )
        .unwrap();
        assert!(resp["result"]["content"][0]["text"].as_str().is_some());
    }

    #[test]
    fn tools_call_unknown_tool_returns_error() {
        let resp = req(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"bogus","arguments":{}}}"#,
        )
        .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }
}
