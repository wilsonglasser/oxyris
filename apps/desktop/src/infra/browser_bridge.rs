//! TCP control bridge that lets the out-of-process MCP server drive the shared
//! headless browser ([`oxyris_browser::BrowserManager`]) in the desktop.
//! Mirrors [`crate::infra::autopilot_bridge`] / [`crate::infra::lsp_bridge`] —
//! line-delimited JSON-RPC 2.0 over a `127.0.0.1:<random>` socket bound at boot.
//!
//! The MCP server is told the port via `--browser-bridge tcp://…` and exposes
//! the `browser_*` tools that forward here. One browser is shared across every
//! Claude session and the auto-pilot.
//!
//! Methods: `browser.navigate{url}`, `browser.screenshot{}` → `{data:<b64 png>}`,
//! `browser.click{selector}`, `browser.type{selector,text}`,
//! `browser.eval{expression}` → `{value}`, `browser.snapshot{}` → `{text}`,
//! `browser.wait_for{selector,timeout_ms?}`.

use std::sync::Arc;
use std::time::Duration;

use oxyris_browser::BrowserManager;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Bind the listener and spawn the accept loop. Returns the bound port.
pub async fn serve(browser: Arc<BrowserManager>) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    tracing::info!(port, "browser_bridge: listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let browser = browser.clone();
                    tokio::spawn(handle_conn(stream, browser));
                }
                Err(e) => {
                    tracing::debug!(error = %e, "browser_bridge: accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(port)
}

async fn handle_conn(stream: TcpStream, browser: Arc<BrowserManager>) {
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
        let response = handle_request(trimmed, &browser).await;
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

async fn handle_request(line: &str, browser: &Arc<BrowserManager>) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return rpc_err(Value::Null, -32700, format!("parse error: {e}")),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result = dispatch(browser, method, &params).await;
    match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(message) => rpc_err(id, -32603, message),
    }
}

async fn dispatch(
    browser: &Arc<BrowserManager>,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let err = |e: oxyris_browser::BrowserError| e.to_string();
    match method {
        "browser.navigate" => {
            let url = str_param(params, "url")?;
            browser.navigate(url).await.map_err(err)?;
            Ok(json!({ "ok": true }))
        }
        "browser.screenshot" => {
            let data = browser.screenshot_base64().await.map_err(err)?;
            Ok(json!({ "data": data }))
        }
        "browser.click" => {
            let selector = str_param(params, "selector")?;
            browser.click(selector).await.map_err(err)?;
            Ok(json!({ "ok": true }))
        }
        "browser.type" => {
            let selector = str_param(params, "selector")?;
            let text = str_param(params, "text")?;
            browser.type_text(selector, text).await.map_err(err)?;
            Ok(json!({ "ok": true }))
        }
        "browser.eval" => {
            let expression = str_param(params, "expression")?;
            let value = browser.eval(expression).await.map_err(err)?;
            Ok(json!({ "value": value }))
        }
        "browser.snapshot" => {
            let text = browser.snapshot_text().await.map_err(err)?;
            Ok(json!({ "text": text }))
        }
        "browser.wait_for" => {
            let selector = str_param(params, "selector")?;
            let timeout_ms = params
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(10_000);
            browser
                .wait_for(selector, Duration::from_millis(timeout_ms))
                .await
                .map_err(err)?;
            Ok(json!({ "ok": true }))
        }
        "ping" => Ok(json!({})),
        other => Err(format!("unknown method: {other}")),
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing '{key}'"))
}

fn rpc_err(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
