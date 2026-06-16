//! TCP control bridge that lets the out-of-process MCP server engage / disengage
//! the auto-pilot in the desktop. Mirrors [`crate::infra::lsp_bridge`] — line-
//! delimited JSON-RPC 2.0 over a `127.0.0.1:<random>` socket bound at boot.
//!
//! The MCP server (a child of `claude`) is told the port via
//! `--autopilot-bridge tcp://…` and the **calling session's id** via
//! `--session-id`, both baked into its per-session `mcp-<session>.json`. So the
//! `oxyris_autopilot_engage` tool hands the wheel to the pilot for *its own*
//! session without the frontend in the loop, and without Claude being able to
//! target a different session (the id is not a tool argument — it's baked).
//!
//! Methods:
//! - `autopilot.engage({session_id, mission})` → engages with the persisted
//!   default supervisor config (see [`crate::infra::autopilot_config`]).
//! - `autopilot.disengage({session_id})`

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oxyris_core::AggregateId;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::infra::autopilot::AutopilotManager;
use crate::infra::autopilot_config;

/// Bind the bridge listener and spawn the accept loop. Returns the bound port so
/// the caller can wire it into the per-session `mcp.json`. `data_dir` is where
/// the persisted supervisor defaults live — read fresh on each engage so a
/// Settings change applies without restarting the bridge.
pub async fn serve(autopilot: Arc<AutopilotManager>, data_dir: PathBuf) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    tracing::info!(port, "autopilot_bridge: listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let autopilot = autopilot.clone();
                    let data_dir = data_dir.clone();
                    tokio::spawn(handle_conn(stream, autopilot, data_dir));
                }
                Err(e) => {
                    tracing::debug!(error = %e, "autopilot_bridge: accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(port)
}

async fn handle_conn(stream: TcpStream, autopilot: Arc<AutopilotManager>, data_dir: PathBuf) {
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
        let response = handle_request(trimmed, &autopilot, &data_dir).await;
        let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
        bytes.push(b'\n');
        if write.write_all(&bytes).await.is_err() {
            break;
        }
        if write.flush().await.is_err() {
            break;
        }
    }
}

async fn handle_request(line: &str, autopilot: &Arc<AutopilotManager>, data_dir: &Path) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return rpc_err(Value::Null, -32700, format!("parse error: {e}")),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "autopilot.engage" => engage(autopilot, data_dir, &params).await,
        "autopilot.disengage" => disengage(autopilot, &params).await,
        "ping" => Ok(json!({})),
        other => Err(format!("unknown method: {other}")),
    };

    match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(message) => rpc_err(id, -32603, message),
    }
}

fn rpc_err(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn parse_session_id(params: &Value) -> Result<AggregateId, String> {
    let s = params
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'session_id'".to_string())?;
    // AggregateId is `#[serde(transparent)]` over a Uuid — parse via serde so we
    // don't take a direct uuid dependency here.
    serde_json::from_value(Value::String(s.to_string()))
        .map_err(|e| format!("invalid session_id: {e}"))
}

async fn engage(
    autopilot: &Arc<AutopilotManager>,
    data_dir: &Path,
    params: &Value,
) -> Result<Value, String> {
    let session_id = parse_session_id(params)?;
    let mission = params
        .get("mission")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'mission'".to_string())?;
    if mission.trim().is_empty() {
        return Err("mission is empty".into());
    }
    let (config, max_turns) = autopilot_config::load(data_dir).to_config()?;
    autopilot
        .engage(session_id, mission.to_string(), config, max_turns)
        .await?;
    Ok(json!({ "engaged": true }))
}

async fn disengage(autopilot: &Arc<AutopilotManager>, params: &Value) -> Result<Value, String> {
    let session_id = parse_session_id(params)?;
    autopilot.disengage(session_id).await;
    Ok(json!({ "disengaged": true }))
}
