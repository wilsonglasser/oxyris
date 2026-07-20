//! TCP control bridge that lets the out-of-process MCP server (running under an
//! **Assistant / Oxy** session) enumerate and drive every OTHER open thread.
//! Mirrors [`crate::infra::autopilot_bridge`] — line-delimited JSON-RPC 2.0 over
//! a `127.0.0.1:<random>` socket bound at boot.
//!
//! Unlike the autopilot bridge, the target thread is a **method parameter**
//! (`thread_id`), not baked into argv: Oxy's whole job is to reach across all
//! threads, so it can target any of them.
//!
//! Methods:
//! - `threads.list({})` → every currently-running thread (id/title/status/turns).
//! - `thread.read({thread_id})` → one thread's full snapshot.
//! - `thread.send({thread_id, text})` → start a turn in that thread; returns
//!   `{turn_id}`.

use std::sync::Arc;
use std::time::Duration;

use oxyris_core::AggregateId;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::infra::session_supervisor::SessionSupervisor;

/// Bind the bridge listener and spawn the accept loop. Returns the bound port so
/// the caller can wire it into an Assistant session's `mcp.json` via
/// `--oxy-bridge`.
pub async fn serve(supervisor: Arc<SessionSupervisor>) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    tracing::info!(port, "oxy_bridge: listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let supervisor = supervisor.clone();
                    tokio::spawn(handle_conn(stream, supervisor));
                }
                Err(e) => {
                    tracing::debug!(error = %e, "oxy_bridge: accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(port)
}

async fn handle_conn(stream: TcpStream, supervisor: Arc<SessionSupervisor>) {
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
        let response = handle_request(trimmed, &supervisor).await;
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

async fn handle_request(line: &str, supervisor: &Arc<SessionSupervisor>) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return rpc_err(Value::Null, -32700, format!("parse error: {e}")),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "threads.list" => threads_list(supervisor).await,
        "thread.read" => thread_read(supervisor, &params).await,
        "thread.send" => thread_send(supervisor, &params).await,
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

fn parse_thread_id(params: &Value) -> Result<AggregateId, String> {
    let s = params
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'thread_id'".to_string())?;
    serde_json::from_value(Value::String(s.to_string()))
        .map_err(|e| format!("invalid thread_id: {e}"))
}

async fn threads_list(supervisor: &Arc<SessionSupervisor>) -> Result<Value, String> {
    let rows = supervisor.list_open_threads().map_err(|e| e.to_string())?;
    let threads: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "thread_id": r.id.to_string(),
                "title": r.title,
                "status": r.status,
                "turn_count": r.turn_count,
            })
        })
        .collect();
    Ok(json!({ "threads": threads }))
}

async fn thread_read(supervisor: &Arc<SessionSupervisor>, params: &Value) -> Result<Value, String> {
    let id = parse_thread_id(params)?;
    match supervisor.read_thread(id).map_err(|e| e.to_string())? {
        Some(snap) => serde_json::to_value(&snap).map_err(|e| e.to_string()),
        None => Err(format!("thread not found: {id}")),
    }
}

async fn thread_send(supervisor: &Arc<SessionSupervisor>, params: &Value) -> Result<Value, String> {
    let id = parse_thread_id(params)?;
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'text'".to_string())?;
    if text.trim().is_empty() {
        return Err("text is empty".into());
    }
    let turn_id = supervisor
        .send_user_message(id, text.to_string())
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "turn_id": turn_id }))
}
