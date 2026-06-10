use std::sync::OnceLock;

use oxyris_ipc::{EventFrame, Frame};
use tokio::io::{AsyncWriteExt, stdout};
use tokio::sync::Mutex;

/// Serializes whole-frame writes to stdout. With fs.watch streaming events from
/// background tasks concurrently with the request loop, two writers could
/// otherwise interleave bytes and corrupt the NDJSON stream.
fn write_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// Write one frame as a single NDJSON line on stdout. Errors here mean the
/// backend went away — we log and keep going; the main loop will hit EOF soon
/// enough.
pub async fn write(frame: &Frame) {
    let mut out = match serde_json::to_vec(frame) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to serialize outgoing frame");
            return;
        }
    };
    out.push(b'\n');

    let _guard = write_lock().lock().await;
    let mut stdout = stdout();
    if let Err(e) = stdout.write_all(&out).await {
        tracing::error!(error = %e, "failed to write frame to stdout");
    }
    let _ = stdout.flush().await;
}

pub async fn emit_event(request_id: &str, data: serde_json::Value) {
    write(&Frame::Event(EventFrame {
        request_id: request_id.to_owned(),
        data,
    }))
    .await;
}
