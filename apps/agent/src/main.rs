//! Oxyris WSL agent.
//!
//! Long-running process the desktop backend deploys into each WSL distro.
//! Reads NDJSON request frames on stdin, executes them, and writes event/result
//! frames back on stdout. All operations run with the agent's own uid inside
//! the distro — same as if the user had opened a shell there. (See `PLAN.md` §5.)

mod ops;
mod protocol;

use anyhow::Result;
use oxyris_ipc::ops::op_name;
use oxyris_ipc::{ErrorFrame, Frame, RequestFrame, ResultFrame};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Logs must go to stderr so stdout stays a clean NDJSON pipe.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("oxyris-agent v{} ready", env!("CARGO_PKG_VERSION"));

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let frame: Frame = match serde_json::from_str(&line) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, raw = %line, "dropping malformed frame");
                continue;
            }
        };

        match frame {
            Frame::Request(req) => handle_request(req).await,
            other => tracing::warn!(?other, "unexpected frame kind from backend; ignoring"),
        }
    }

    tracing::info!("oxyris-agent shutting down (stdin closed)");
    Ok(())
}

async fn handle_request(req: RequestFrame) {
    // fs.watch is special: it keeps its request open and streams change events
    // under this request id until fs.unwatch cancels it. Emitting a Result
    // frame here would make the backend drop the event route, so we start the
    // watcher and return without one. Errors (e.g. bad root) still surface as
    // an Error frame.
    if req.op == op_name::FS_WATCH {
        let id = req.id.clone();
        let args: oxyris_ipc::ops::FsWatchArgs = match serde_json::from_value(req.args) {
            Ok(a) => a,
            Err(e) => {
                protocol::write(&Frame::Error(ErrorFrame {
                    request_id: id,
                    code: "invalid_args".to_owned(),
                    message: e.to_string(),
                }))
                .await;
                return;
            }
        };
        if let Err(e) = ops::start_watch(&id, args).await {
            protocol::write(&Frame::Error(ErrorFrame {
                request_id: id,
                code: e.code().to_owned(),
                message: e.to_string(),
            }))
            .await;
        }
        return;
    }

    let id = req.id.clone();
    match ops::dispatch(req).await {
        Ok(data) => {
            protocol::write(&Frame::Result(ResultFrame {
                request_id: id,
                data,
            }))
            .await
        }
        Err(e) => {
            tracing::warn!(error = %e, "op dispatch failed");
            protocol::write(&Frame::Error(ErrorFrame {
                request_id: id,
                code: e.code().to_owned(),
                message: e.to_string(),
            }))
            .await;
        }
    }
}
