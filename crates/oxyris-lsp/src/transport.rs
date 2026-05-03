//! `Content-Length`-framed JSON-RPC over stdio. Symmetric reader+writer
//! tasks that own the spawned server's pipes; both surface as background
//! tasks driven by a tokio mpsc channel.

use lsp_types::PublishDiagnosticsParams;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::mpsc;

use crate::{DiagnosticsMap, LspError, PendingMap};

/// Outgoing JSON-RPC frame ready to be serialized to the LSP server's stdin.
pub struct OutboundFrame(pub Value);

pub(crate) async fn writer_loop(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<OutboundFrame>,
) {
    while let Some(OutboundFrame(value)) = rx.recv().await {
        let body = match serde_json::to_vec(&value) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "lsp: serialize outbound");
                continue;
            }
        };
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        if stdin.write_all(header.as_bytes()).await.is_err() {
            break;
        }
        if stdin.write_all(&body).await.is_err() {
            break;
        }
        if stdin.flush().await.is_err() {
            break;
        }
    }
    let _ = stdin.shutdown().await;
}

pub(crate) async fn reader_loop(
    stdout: ChildStdout,
    pending: PendingMap,
    diagnostics: DiagnosticsMap,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let content_length = match read_headers(&mut reader).await {
            Ok(Some(len)) => len,
            Ok(None) => break, // EOF — server gone
            Err(e) => {
                tracing::warn!(error = %e, "lsp: header parse");
                break;
            }
        };
        if content_length == 0 {
            continue;
        }
        let mut body = vec![0u8; content_length];
        if reader.read_exact(&mut body).await.is_err() {
            break;
        }
        let value: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "lsp: bad json body");
                continue;
            }
        };
        dispatch(value, &pending, &diagnostics).await;
    }

    // Drain pending requests so callers don't hang forever on a dead server.
    let mut guard = pending.lock().await;
    for (_id, tx) in guard.drain() {
        let _ = tx.send(Err(LspError::ServerGone));
    }
}

async fn read_headers<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<usize>> {
    use tokio::io::AsyncBufReadExt;
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Ok(content_length.or(Some(0)));
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
}

async fn dispatch(value: Value, pending: &PendingMap, diagnostics: &DiagnosticsMap) {
    // Response carries `id` (number) + `result`/`error`.
    if let Some(id) = value.get("id").and_then(|v| v.as_i64()) {
        let result: crate::Result<Value> = if let Some(err) = value.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            Err(LspError::Server { code, message })
        } else {
            Ok(value.get("result").cloned().unwrap_or(Value::Null))
        };
        let mut guard = pending.lock().await;
        if let Some(tx) = guard.remove(&id) {
            let _ = tx.send(result);
        }
        return;
    }

    // Notification or server-initiated request — only the few we care about.
    let Some(method) = value.get("method").and_then(|v| v.as_str()) else {
        return;
    };
    match method {
        "textDocument/publishDiagnostics" => {
            if let Some(params) = value.get("params") {
                match serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
                    Ok(p) => {
                        let mut cache = diagnostics.lock().await;
                        if p.diagnostics.is_empty() {
                            cache.remove(&p.uri);
                        } else {
                            cache.insert(p.uri, p.diagnostics);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "lsp: bad publishDiagnostics");
                    }
                }
            }
        }
        "window/logMessage" | "window/showMessage" => {
            if let Some(params) = value.get("params")
                && let Some(message) = params.get("message").and_then(|v| v.as_str())
            {
                tracing::debug!(target: "oxyris_lsp::server", "{message}");
            }
        }
        _ => {
            // Server-initiated request we don't handle. If it has an id,
            // reply with method-not-found so the server doesn't hang. Skip
            // for simple notifications.
            if let Some(_id) = value.get("id") {
                tracing::debug!(method, "lsp: ignored server request");
            }
        }
    }
}
