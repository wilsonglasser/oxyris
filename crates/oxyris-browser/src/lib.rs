//! Headless browser automation over CDP, shared by Claude Code (via MCP tools)
//! and the auto-pilot for navigating, interacting, and screenshotting a page to
//! validate work. Drives a localhost headless Edge — see [`launch`] — through a
//! hand-rolled CDP client — see [`cdp`].
//!
//! [`BrowserManager`] owns at most one browser session, launched lazily on the
//! first op and reused after. All ops are `&self` so it lives behind an `Arc`
//! in the desktop's app state.

mod cdp;
mod launch;

use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::cdp::CdpClient;

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// No usable browser binary / couldn't start one. The tools should report
    /// this as "browser unavailable" rather than a hard failure.
    #[error("browser unavailable: {0}")]
    NotAvailable(String),
    /// A CDP-level failure (transport, protocol, or a method error).
    #[error("cdp: {0}")]
    Cdp(String),
    /// JavaScript thrown by an `eval` / `click` / `type` op.
    #[error("page script error: {0}")]
    Script(String),
}

struct Session {
    child: Child,
    cdp: Arc<CdpClient>,
    #[allow(dead_code)]
    port: u16,
    user_data_dir: PathBuf,
}

/// Owns the single shared browser session. Launches on first use.
pub struct BrowserManager {
    session: Mutex<Option<Session>>,
}

impl Default for BrowserManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
        }
    }

    /// Get the live CDP client, launching + initializing the browser if needed.
    async fn client(&self) -> Result<Arc<CdpClient>, BrowserError> {
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.cdp.clone());
        }
        let launched = launch::launch().await?;
        let cdp = CdpClient::connect(&launched.ws_url).await?;
        // Enable the domains our ops use. Best-effort: a missing one surfaces
        // when the dependent op runs.
        let _ = cdp.call("Page.enable", json!({})).await;
        let _ = cdp.call("Runtime.enable", json!({})).await;
        *guard = Some(Session {
            child: launched.child,
            cdp: cdp.clone(),
            port: launched.port,
            user_data_dir: launched.user_data_dir,
        });
        Ok(cdp)
    }

    /// Navigate to `url` and wait for the document to finish loading.
    pub async fn navigate(&self, url: &str) -> Result<(), BrowserError> {
        let cdp = self.client().await?;
        let res = cdp.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(err) = res.get("errorText").and_then(|v| v.as_str()) {
            return Err(BrowserError::Cdp(format!("navigate {url}: {err}")));
        }
        self.wait_ready(&cdp, Duration::from_secs(15)).await
    }

    /// Capture a full-viewport PNG, returned as the raw base64 string CDP gives
    /// us — callers that need an MCP image block can pass it straight through;
    /// UI/disk consumers decode it.
    pub async fn screenshot_base64(&self) -> Result<String, BrowserError> {
        let cdp = self.client().await?;
        let res = cdp
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        res.get("data")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| BrowserError::Cdp("captureScreenshot: no data".into()))
    }

    /// Evaluate a JavaScript expression, returning its value. Promises are
    /// awaited; a thrown error becomes [`BrowserError::Script`].
    pub async fn eval(&self, expression: &str) -> Result<Value, BrowserError> {
        let cdp = self.client().await?;
        self.eval_on(&cdp, expression).await
    }

    /// Click the first element matching a CSS selector.
    pub async fn click(&self, selector: &str) -> Result<(), BrowserError> {
        let sel = js_str(selector);
        let expr = format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) throw new Error('selector not found: ' + {sel}); el.click(); return true; }})()"
        );
        self.eval(&expr).await.map(|_| ())
    }

    /// Set the value of a form field (by selector) and fire input/change so
    /// frameworks notice. Faithful enough for validation flows; not a key-by-key
    /// simulation.
    pub async fn type_text(&self, selector: &str, text: &str) -> Result<(), BrowserError> {
        let sel = js_str(selector);
        let txt = js_str(text);
        let expr = format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) throw new Error('selector not found: ' + {sel}); el.focus(); el.value = {txt}; el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); return true; }})()"
        );
        self.eval(&expr).await.map(|_| ())
    }

    /// The page's visible text (`document.body.innerText`) — a cheap content
    /// snapshot for the model to reason about without a screenshot.
    pub async fn snapshot_text(&self) -> Result<String, BrowserError> {
        let v = self
            .eval("document.body ? document.body.innerText : ''")
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// Poll until a selector matches or the timeout elapses.
    pub async fn wait_for(&self, selector: &str, timeout: Duration) -> Result<(), BrowserError> {
        let cdp = self.client().await?;
        let sel = js_str(selector);
        let expr = format!("!!document.querySelector({sel})");
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.eval_on(&cdp, &expr).await?.as_bool() == Some(true) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(BrowserError::Script(format!(
                    "selector not found within timeout: {selector}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Shut the browser down and drop the session.
    pub async fn shutdown(&self) {
        if let Some(mut s) = self.session.lock().await.take() {
            let _ = s.child.kill();
            // The temp profile is disposable; remove it best-effort.
            let _ = std::fs::remove_dir_all(&s.user_data_dir);
        }
    }

    async fn eval_on(&self, cdp: &Arc<CdpClient>, expression: &str) -> Result<Value, BrowserError> {
        let res = cdp
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(exc) = res.get("exceptionDetails") {
            let msg = exc
                .get("exception")
                .and_then(|e| e.get("description").or_else(|| e.get("value")))
                .and_then(|v| v.as_str())
                .or_else(|| exc.get("text").and_then(|v| v.as_str()))
                .unwrap_or("script threw");
            return Err(BrowserError::Script(msg.to_string()));
        }
        Ok(res
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn wait_ready(
        &self,
        cdp: &Arc<CdpClient>,
        timeout: Duration,
    ) -> Result<(), BrowserError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let state = self.eval_on(cdp, "document.readyState").await?;
            if state.as_str() == Some("complete") {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                // Not fatal — return Ok so callers can still screenshot a
                // partially-loaded page rather than erroring outright.
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// JSON-encode a string so it can be safely embedded as a JS string literal
/// (handles quotes, backslashes, newlines).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_str_escapes() {
        assert_eq!(js_str("a\"b"), "\"a\\\"b\"");
        assert_eq!(js_str("#id .cls"), "\"#id .cls\"");
    }

    // Real end-to-end against a headless Edge. Ignored by default (needs Edge +
    // is slow); run with `cargo test -p oxyris-browser -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_navigate_eval_screenshot() {
        let b = BrowserManager::new();
        b.navigate("https://example.com").await.expect("navigate");
        let title = b.eval("document.title").await.expect("eval");
        assert_eq!(title.as_str(), Some("Example Domain"));
        let shot = b.screenshot_base64().await.expect("screenshot");
        assert!(shot.len() > 1000, "screenshot too small: {}", shot.len());
        b.shutdown().await;
    }
}
