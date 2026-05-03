//! TCP client for the desktop's LSP bridge. Each call opens a fresh
//! connection — one round-trip, no multiplexing, no connection pool. LSP
//! latency dominates; the TCP setup is in the noise.
//!
//! Wire format must match `apps/desktop/src/infra/lsp_bridge.rs`:
//! line-delimited JSON-RPC 2.0. Every request includes the `workspace`
//! absolute path so the desktop can route to the right shared client.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use oxyris_lsp::lsp_types::{Diagnostic, Location};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub struct LspBridgeClient {
    address: String,
    workspace: PathBuf,
    next_id: AtomicI64,
}

impl LspBridgeClient {
    /// `address` is `tcp://host:port` (or just `host:port`). `workspace`
    /// is the absolute path the MCP server was launched with — every
    /// bridge request includes it so the desktop knows which LSP pool to
    /// route to.
    pub fn new(address: &str, workspace: PathBuf) -> Self {
        let address = address.trim_start_matches("tcp://").to_owned();
        Self {
            address,
            workspace,
            next_id: AtomicI64::new(1),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub async fn find_references(
        &self,
        file: &Path,
        line: u32,
        column: u32,
        include_declaration: bool,
    ) -> Result<Vec<Location>, String> {
        let result = self
            .call(
                "lsp.find_references",
                json!({
                    "workspace": self.workspace.to_string_lossy(),
                    "file": file.to_string_lossy(),
                    "line": line,
                    "column": column,
                    "include_declaration": include_declaration,
                }),
            )
            .await?;
        serde_json::from_value::<Vec<Location>>(result).map_err(|e| e.to_string())
    }

    pub async fn hover(
        &self,
        file: &Path,
        line: u32,
        column: u32,
    ) -> Result<Option<String>, String> {
        let result = self
            .call(
                "lsp.hover",
                json!({
                    "workspace": self.workspace.to_string_lossy(),
                    "file": file.to_string_lossy(),
                    "line": line,
                    "column": column,
                }),
            )
            .await?;
        serde_json::from_value::<Option<String>>(result).map_err(|e| e.to_string())
    }

    pub async fn diagnostics(&self, file: &Path) -> Result<Vec<Diagnostic>, String> {
        let result = self
            .call(
                "lsp.diagnostics",
                json!({
                    "workspace": self.workspace.to_string_lossy(),
                    "file": file.to_string_lossy(),
                }),
            )
            .await?;
        serde_json::from_value::<Vec<Diagnostic>>(result).map_err(|e| e.to_string())
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let stream = TcpStream::connect(&self.address)
            .await
            .map_err(|e| format!("connect {}: {e}", self.address))?;
        let (read, mut write) = stream.into_split();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut bytes = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        write
            .write_all(&bytes)
            .await
            .map_err(|e| format!("write: {e}"))?;
        write.flush().await.map_err(|e| format!("flush: {e}"))?;

        let mut reader = BufReader::new(read);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("read: {e}"))?;
        let resp: Value =
            serde_json::from_str(line.trim()).map_err(|e| format!("parse: {e} (line: {line})"))?;
        if let Some(err) = resp.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("bridge error");
            return Err(msg.to_owned());
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }
}
