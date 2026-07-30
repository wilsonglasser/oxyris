//! `Content-Length`-framed JSON-RPC over stdio. Symmetric reader+writer
//! tasks that own the spawned server's pipes; both surface as background
//! tasks driven by a tokio mpsc channel.

use std::collections::BTreeSet;

use lsp_types::PublishDiagnosticsParams;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, watch};

use crate::{DiagnosticsMap, LspError, PendingMap};

/// Server-reported long-running work, tracked from `$/progress`. Waiters use
/// it to block until an async job the server started on our behalf is done —
/// specifically rust-analyzer's flycheck (`cargo check`), which is what makes
/// check-layer diagnostics readable instead of a race.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    /// Tokens with a `begin` but no `end` yet.
    pub active: BTreeSet<String>,
    /// Monotonic count of *flycheck* `end` notifications. Lets a waiter detect
    /// a check that began **and** ended between two observations of `active`.
    ///
    /// Counting every token instead would be wrong in a way that silently
    /// defeats the whole wait: rust-analyzer reports indexing, cache priming
    /// and workspace loading through the same mechanism, so any of those
    /// finishing would look like "the check is done".
    pub flycheck_completions: u64,
}

impl ProgressSnapshot {
    /// True when a flycheck-ish job is in flight. rust-analyzer names its
    /// token `rustAnalyzer/Flycheck`; match loosely so a renamed token (or a
    /// `clippy`-flavoured one) still counts.
    pub fn flycheck_running(&self) -> bool {
        self.active.iter().any(|t| is_flycheck_token(t))
    }
}

pub(crate) type ProgressTx = watch::Sender<ProgressSnapshot>;

pub(crate) fn is_flycheck_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    lower.contains("flycheck") || lower.contains("cargo check") || lower.contains("clippy")
}

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
    progress: ProgressTx,
    // Same channel the client writes on — the reader needs it to answer
    // server-initiated requests.
    out: mpsc::UnboundedSender<OutboundFrame>,
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
        dispatch(value, &pending, &diagnostics, &progress, &out).await;
    }

    // Drain pending requests so callers don't hang forever on a dead server.
    let mut guard = pending.lock().await;
    for (_id, tx) in guard.drain() {
        let _ = tx.send(Err(LspError::ServerGone));
    }
    // Nothing can finish on a dead server — clear in-flight work so a waiter
    // blocked on flycheck gives up now instead of at its timeout.
    progress.send_modify(|s| {
        s.active.clear();
        s.flycheck_completions = s.flycheck_completions.wrapping_add(1);
    });
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

fn reply_ok(out: &mpsc::UnboundedSender<OutboundFrame>, id: Value) {
    let _ = out.send(OutboundFrame(
        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }),
    ));
}

fn reply_method_not_found(out: &mpsc::UnboundedSender<OutboundFrame>, id: Value, method: &str) {
    let _ = out.send(OutboundFrame(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32601, "message": format!("unsupported by oxyris-lsp: {method}") },
    })));
}

async fn dispatch(
    value: Value,
    pending: &PendingMap,
    diagnostics: &DiagnosticsMap,
    progress: &ProgressTx,
    out: &mpsc::UnboundedSender<OutboundFrame>,
) {
    // Requests and notifications carry `method`; responses never do. Keying off
    // `id` instead would misread a server-initiated *request* with a numeric id
    // (rust-analyzer's `window/workDoneProgress/create`) as a response to
    // something we sent, and drop it unanswered.
    let method = value.get("method").and_then(|v| v.as_str());

    if method.is_none()
        && let Some(id) = value.get("id").and_then(|v| v.as_i64())
    {
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
    let Some(method) = method else {
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
        "$/progress" => {
            let Some(params) = value.get("params") else {
                return;
            };
            let token = match params.get("token") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return,
            };
            match params
                .get("value")
                .and_then(|v| v.get("kind"))
                .and_then(|v| v.as_str())
            {
                Some("begin") => progress.send_modify(|s| {
                    s.active.insert(token);
                }),
                Some("end") => progress.send_modify(|s| {
                    s.active.remove(&token);
                    if is_flycheck_token(&token) {
                        s.flycheck_completions = s.flycheck_completions.wrapping_add(1);
                    }
                }),
                // `report` is pure noise for us — we only need begin/end edges.
                _ => {}
            }
        }
        "window/logMessage" | "window/showMessage" => {
            if let Some(params) = value.get("params")
                && let Some(message) = params.get("message").and_then(|v| v.as_str())
            {
                tracing::debug!(target: "oxyris_lsp::server", "{message}");
            }
        }
        // Server-initiated requests we must answer. `workDoneProgress/create`
        // is the handshake in front of every `$/progress` stream — leaving it
        // unanswered leaves the server waiting on a reply that never comes.
        // We accept unconditionally; the tokens are what we actually want.
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability" => {
            if let Some(id) = value.get("id") {
                reply_ok(out, id.clone());
            }
        }
        _ => {
            // Anything else the server asks for, we decline explicitly rather
            // than silently — an unanswered request can stall the server.
            if let Some(id) = value.get("id") {
                tracing::debug!(method, "lsp: declining server request");
                reply_method_not_found(out, id.clone(), method);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    type Harness = (
        PendingMap,
        DiagnosticsMap,
        ProgressTx,
        mpsc::UnboundedSender<OutboundFrame>,
        mpsc::UnboundedReceiver<OutboundFrame>,
    );

    fn harness() -> Harness {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        (
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            watch::channel(ProgressSnapshot::default()).0,
            out_tx,
            out_rx,
        )
    }

    #[tokio::test]
    async fn progress_begin_and_end_track_flycheck() {
        let (pending, diags, progress, out, _out_rx) = harness();
        let begin = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "$/progress",
            "params": { "token": "rustAnalyzer/Flycheck", "value": { "kind": "begin", "title": "cargo check" } },
        });
        dispatch(begin, &pending, &diags, &progress, &out).await;
        assert!(progress.borrow().flycheck_running());
        assert_eq!(progress.borrow().flycheck_completions, 0);

        let report = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "$/progress",
            "params": { "token": "rustAnalyzer/Flycheck", "value": { "kind": "report", "message": "1/9" } },
        });
        dispatch(report, &pending, &diags, &progress, &out).await;
        assert!(
            progress.borrow().flycheck_running(),
            "report must not end it"
        );

        let end = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "$/progress",
            "params": { "token": "rustAnalyzer/Flycheck", "value": { "kind": "end" } },
        });
        dispatch(end, &pending, &diags, &progress, &out).await;
        assert!(!progress.borrow().flycheck_running());
        assert_eq!(progress.borrow().flycheck_completions, 1);
    }

    #[tokio::test]
    async fn numeric_progress_tokens_are_tracked() {
        let (pending, diags, progress, out, _out_rx) = harness();
        for kind in ["begin", "end"] {
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "$/progress",
                "params": { "token": 7, "value": { "kind": kind } },
            });
            dispatch(frame, &pending, &diags, &progress, &out).await;
        }
        assert!(
            progress.borrow().active.is_empty(),
            "token 7 opened and closed"
        );
    }

    /// The bug this guards: rust-analyzer finishes indexing constantly, and if
    /// that counted as a completion then `wait_for_flycheck` would return
    /// immediately with the *previous* check's diagnostics.
    #[tokio::test]
    async fn non_flycheck_completions_are_not_counted() {
        let (pending, diags, progress, out, _out_rx) = harness();
        for token in ["rustAnalyzer/Indexing", "rustAnalyzer/cachePriming"] {
            for kind in ["begin", "end"] {
                let frame = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "$/progress",
                    "params": { "token": token, "value": { "kind": kind } },
                });
                dispatch(frame, &pending, &diags, &progress, &out).await;
            }
        }
        assert_eq!(
            progress.borrow().flycheck_completions,
            0,
            "only a flycheck end may count as a completed check"
        );
    }

    #[tokio::test]
    async fn work_done_progress_create_is_answered() {
        let (pending, diags, progress, out, mut out_rx) = harness();
        let create = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "window/workDoneProgress/create",
            "params": { "token": "rustAnalyzer/Flycheck" },
        });
        dispatch(create, &pending, &diags, &progress, &out).await;
        let OutboundFrame(reply) = out_rx
            .try_recv()
            .expect("a numeric-id server request must still be answered");
        assert_eq!(reply["id"], 3);
        assert!(reply["result"].is_null());
        assert!(reply.get("error").is_none(), "must accept, not decline");
    }

    #[tokio::test]
    async fn unknown_server_request_is_declined_not_ignored() {
        let (pending, diags, progress, out, mut out_rx) = harness();
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "abc",
            "method": "workspace/inlayHint/refresh",
        });
        dispatch(req, &pending, &diags, &progress, &out).await;
        let OutboundFrame(reply) = out_rx.try_recv().expect("a reply was sent");
        assert_eq!(reply["id"], "abc");
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn notifications_never_get_a_reply() {
        let (pending, diags, progress, out, mut out_rx) = harness();
        let note = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": 3, "message": "hello" },
        });
        dispatch(note, &pending, &diags, &progress, &out).await;
        assert!(
            out_rx.try_recv().is_err(),
            "a notification has no id to answer"
        );
    }

    #[test]
    fn flycheck_token_matching_is_narrow() {
        assert!(is_flycheck_token("rustAnalyzer/Flycheck"));
        assert!(is_flycheck_token("cargo check"));
        assert!(is_flycheck_token("clippy"));
        assert!(!is_flycheck_token("rustAnalyzer/Indexing"));
        assert!(!is_flycheck_token("rustAnalyzer/cachePriming"));
        assert!(!is_flycheck_token("rustAnalyzer/Roots Scanned"));
    }
}
