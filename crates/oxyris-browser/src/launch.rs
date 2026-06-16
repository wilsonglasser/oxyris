//! Locate and launch a headless Edge (the WebView2 Chromium, already on every
//! Windows box — no bundled browser) with a CDP debugging port, then discover
//! the page target's WebSocket URL. Confirmed working against Edge 149 /
//! `--headless=new` / CDP 1.3.

use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

use serde_json::Value;

use crate::BrowserError;

/// A spawned browser process plus the page endpoint to drive it.
pub struct Launched {
    pub child: Child,
    pub ws_url: String,
    pub port: u16,
    pub user_data_dir: PathBuf,
}

/// Resolve the Edge binary: explicit `OXYRIS_BROWSER_BIN` override wins, then
/// the standard install locations. Returns an error (not a panic) so the
/// browser tools degrade to "not available" rather than taking down the app.
fn resolve_edge() -> Result<PathBuf, BrowserError> {
    if let Some(p) = std::env::var_os("OXYRIS_BROWSER_BIN") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Ok(path);
        }
    }
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];
    for c in candidates {
        let path = PathBuf::from(c);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(BrowserError::NotAvailable(
        "Microsoft Edge not found; set OXYRIS_BROWSER_BIN to a Chromium binary".into(),
    ))
}

/// Grab a free localhost TCP port by binding to 0 and releasing it. A small
/// race window exists before Edge claims it; acceptable for a single launch.
fn free_port() -> Result<u16, BrowserError> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| BrowserError::NotAvailable(format!("no free port: {e}")))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| BrowserError::NotAvailable(e.to_string()))
}

/// Spawn headless Edge and resolve the page WebSocket endpoint.
pub async fn launch() -> Result<Launched, BrowserError> {
    use oxyris_procutil::HideConsole;
    use std::process::Command;

    let bin = resolve_edge()?;
    let port = free_port()?;
    let user_data_dir =
        std::env::temp_dir().join(format!("oxyris-browser-{}", uuid::Uuid::now_v7().simple()));

    let child = Command::new(&bin)
        .arg("--headless=new")
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .arg("--disable-extensions")
        .arg("--disable-sync")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("about:blank")
        .hide_console()
        .spawn()
        .map_err(|e| BrowserError::NotAvailable(format!("spawn edge: {e}")))?;

    // Poll the DevTools HTTP endpoint until the page target shows up (cold
    // start is usually <1s; allow ~5s before giving up).
    let client = reqwest::Client::new();
    let list_url = format!("http://127.0.0.1:{port}/json/list");
    let mut last_err = String::from("no page target");
    for _ in 0..50 {
        match fetch_page_ws(&client, &list_url).await {
            Ok(ws_url) => {
                return Ok(Launched {
                    child,
                    ws_url,
                    port,
                    user_data_dir,
                });
            }
            Err(e) => last_err = e,
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Couldn't reach it — don't leak the process.
    let mut child = child;
    let _ = child.kill();
    Err(BrowserError::NotAvailable(format!(
        "edge devtools endpoint never came up: {last_err}"
    )))
}

/// Fetch `/json/list` and return the first `page` target's debugger URL.
async fn fetch_page_ws(client: &reqwest::Client, list_url: &str) -> Result<String, String> {
    let body = client
        .get(list_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let targets: Vec<Value> = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    targets
        .iter()
        .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
        .and_then(|t| t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .ok_or_else(|| "no page target yet".to_string())
}
