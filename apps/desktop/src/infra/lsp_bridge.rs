//! TCP bridge that lets the out-of-process MCP server proxy LSP calls
//! into the desktop's `LspManager` instead of spawning its own language
//! servers. One rust-analyzer / tsserver / intelephense per worktree,
//! shared across every Claude session in that worktree.
//!
//! Wire format: line-delimited JSON-RPC 2.0 over a long-lived TCP socket
//! on `127.0.0.1:<random>`. The desktop binds at boot, writes the port
//! into `AppState`, and `infra::mcp` injects `--lsp-bridge tcp://…` into
//! the per-worktree `mcp.json`.
//!
//! Methods accepted:
//! - `lsp.find_references({workspace,file,line,column,include_declaration})`
//!   → `[Location]`
//! - `lsp.hover({workspace,file,line,column})` → `Option<String>`
//! - `lsp.diagnostics({workspace,file})` → `[Diagnostic]`
//!
//! `workspace` and `file` are absolute paths. `line`/`column` are 0-based
//! (LSP-native), unlike the MCP tool layer which converts from 1-based.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oxyris_core::Environment;
use serde_json::{Value, json};

use crate::infra::wsl_distro_for_path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::infra::lsp::LspManager;

/// Bind the bridge listener and spawn the accept loop. Returns the bound
/// port so callers can wire it into `mcp.json`.
pub async fn serve(lsp: Arc<LspManager>) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    tracing::info!(port, "lsp_bridge: listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let lsp = lsp.clone();
                    tokio::spawn(handle_conn(stream, addr, lsp));
                }
                Err(e) => {
                    tracing::debug!(error = %e, "lsp_bridge: accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(port)
}

async fn handle_conn(stream: TcpStream, addr: SocketAddr, lsp: Arc<LspManager>) {
    tracing::debug!(?addr, "lsp_bridge: client connected");
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) | Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = handle_request(trimmed, &lsp).await;
        let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
        bytes.push(b'\n');
        if write.write_all(&bytes).await.is_err() {
            break;
        }
        if write.flush().await.is_err() {
            break;
        }
    }
    tracing::debug!(?addr, "lsp_bridge: client disconnected");
}

async fn handle_request(line: &str, lsp: &Arc<LspManager>) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": format!("parse error: {e}") },
            });
        }
    };
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "lsp.find_references" => find_references(lsp, &params).await,
        "lsp.hover" => hover(lsp, &params).await,
        "lsp.diagnostics" => diagnostics(lsp, &params).await,
        "ping" => Ok(json!({})),
        other => Err(format!("unknown method: {other}")),
    };

    match result {
        Ok(value) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": value,
        }),
        Err(message) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32603, "message": message },
        }),
    }
}

async fn find_references(lsp: &Arc<LspManager>, params: &Value) -> Result<Value, String> {
    let (workspace, file, line, column) = parse_position(params)?;
    let include_declaration = params
        .get("include_declaration")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let lang = LspManager::language_for_workspace(&workspace, &file)
        .ok_or_else(|| format!("no LSP language detected for {}", file.display()))?;
    let env = bridge_env_for(&workspace);
    let client = lsp
        .ensure_at(&workspace, &env, lang)
        .await
        .map_err(|e| e.to_string())?;
    let locations = client
        .find_references(&file, line, column, include_declaration)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::to_value(locations).map_err(|e| e.to_string())
}

async fn hover(lsp: &Arc<LspManager>, params: &Value) -> Result<Value, String> {
    let (workspace, file, line, column) = parse_position(params)?;
    let lang = LspManager::language_for_workspace(&workspace, &file)
        .ok_or_else(|| format!("no LSP language detected for {}", file.display()))?;
    let env = bridge_env_for(&workspace);
    let client = lsp
        .ensure_at(&workspace, &env, lang)
        .await
        .map_err(|e| e.to_string())?;
    let result = client
        .hover(&file, line, column)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::to_value(result).map_err(|e| e.to_string())
}

async fn diagnostics(lsp: &Arc<LspManager>, params: &Value) -> Result<Value, String> {
    let workspace = params
        .get("workspace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'workspace'".to_string())?;
    let file = params
        .get("file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'file'".to_string())?;
    let workspace = PathBuf::from(workspace);
    let file = PathBuf::from(file);
    let lang = LspManager::language_for_workspace(&workspace, &file)
        .ok_or_else(|| format!("no LSP language detected for {}", file.display()))?;
    let env = bridge_env_for(&workspace);
    let client = lsp
        .ensure_at(&workspace, &env, lang)
        .await
        .map_err(|e| e.to_string())?;
    client.ensure_open(&file).await.map_err(|e| e.to_string())?;
    let diags = client
        .diagnostics_for(&file)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::to_value(diags).map_err(|e| e.to_string())
}

/// Detect whether a workspace path the bridge received refers to a WSL
/// distro. POSIX-style absolute paths (`/home/...`) are WSL — the desktop
/// translates Windows worktree roots to POSIX before passing to MCP.
/// Anything Windows-shaped (drive letter or UNC) routes as Windows env.
fn bridge_env_for(workspace: &Path) -> Environment {
    let s = workspace.to_string_lossy();
    if s.starts_with('/')
        && let Some(distro) = wsl_distro_for_path(workspace)
    {
        return Environment::Wsl { distro };
    }
    Environment::Local
}

fn parse_position(params: &Value) -> Result<(PathBuf, PathBuf, u32, u32), String> {
    let workspace = params
        .get("workspace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'workspace'".to_string())?;
    let file = params
        .get("file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'file'".to_string())?;
    let line = params
        .get("line")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing 'line' (0-based)".to_string())?;
    let column = params
        .get("column")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing 'column' (0-based)".to_string())?;
    Ok((
        PathBuf::from(workspace),
        PathBuf::from(file),
        line as u32,
        column as u32,
    ))
}
