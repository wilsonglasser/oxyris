//! Minimal Chrome DevTools Protocol client over a single WebSocket.
//!
//! CDP is just JSON messages over `ws://`: requests carry an `id` + `method` +
//! `params` and get a matching `{id, result|error}` back; unsolicited
//! `{method, params}` messages are events. We only need request/response, so a
//! reader task resolves each response to the right caller via a `oneshot` keyed
//! by id, and events are dropped. Mirrors the hand-rolled JSON-RPC in
//! `lsp_bridge` / the MCP server rather than pulling a heavyweight CDP crate.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::stream::{SplitSink, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::BrowserError;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// A live CDP connection. Cloneable callers send through the shared write half;
/// a background task fans responses back out by id.
pub struct CdpClient {
    next_id: AtomicU64,
    sink: Mutex<SplitSink<Ws, Message>>,
    pending: Pending,
}

impl CdpClient {
    /// Connect to a CDP WebSocket endpoint (`ws://host:port/devtools/...`). No
    /// TLS — the endpoint is always a localhost headless browser.
    pub async fn connect(ws_url: &str) -> Result<Arc<Self>, BrowserError> {
        // Parse `ws://authority/path` by hand to avoid a url-crate dep and to
        // open the TcpStream ourselves (so we never need the tungstenite
        // `connect`/TLS features).
        let authority = ws_url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| BrowserError::Cdp(format!("bad ws url: {ws_url}")))?;
        let tcp = TcpStream::connect(authority)
            .await
            .map_err(|e| BrowserError::Cdp(format!("connect {authority}: {e}")))?;
        let (ws, _resp) = tokio_tungstenite::client_async(ws_url, MaybeTlsStream::Plain(tcp))
            .await
            .map_err(|e| BrowserError::Cdp(format!("ws handshake: {e}")))?;

        let (sink, mut stream) = ws.split();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        // Reader task: resolve each `{id,...}` response to its waiting caller;
        // events (`{method,...}` without id) are ignored.
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
                    continue; // event
                };
                if let Some(tx) = reader_pending.lock().await.remove(&id) {
                    let result = match v.get("error") {
                        Some(err) => Err(err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("cdp error")
                            .to_string()),
                        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(result);
                }
            }
            // Connection gone — fail any still-waiting callers instead of
            // hanging them forever.
            let mut map = reader_pending.lock().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Err("cdp connection closed".into()));
            }
        });

        Ok(Arc::new(Self {
            next_id: AtomicU64::new(1),
            sink: Mutex::new(sink),
            pending,
        }))
    }

    /// Issue a CDP method call and await its result. 30s ceiling so a wedged
    /// page can't block a caller indefinitely.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, BrowserError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let frame = json!({ "id": id, "method": method, "params": params });
        let text = serde_json::to_string(&frame).map_err(|e| BrowserError::Cdp(e.to_string()))?;
        {
            let mut sink = self.sink.lock().await;
            sink.send(Message::Text(text))
                .await
                .map_err(|e| BrowserError::Cdp(format!("send {method}: {e}")))?;
        }

        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(msg))) => Err(BrowserError::Cdp(format!("{method}: {msg}"))),
            Ok(Err(_)) => Err(BrowserError::Cdp(format!("{method}: response dropped"))),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(BrowserError::Cdp(format!("{method}: timed out")))
            }
        }
    }
}
