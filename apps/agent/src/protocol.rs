use oxyris_ipc::{EventFrame, Frame};
use tokio::io::{AsyncWriteExt, stdout};

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
