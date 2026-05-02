//! Structured logging: human-friendly lines on stdout + daily-rotating
//! NDJSON files under `<data_dir>/logs/`. Call [`install`] once at app boot;
//! keep the returned guard alive for the life of the process so the
//! background writer flushes its buffer on shutdown.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Holds the non-blocking writer guard; drop it to flush + stop the
/// background log-writer thread.
pub struct LogGuard(#[allow(dead_code)] WorkerGuard);

/// Configure global tracing. Writes:
///
/// - human lines to stderr for dev/running-from-terminal.
/// - NDJSON daily-rotated files under `<logs_dir>/trace.ndjson.YYYY-MM-DD`.
///
/// The env filter respects `RUST_LOG` / `OXYRIS_LOG`; defaults to `info`.
pub fn install(logs_dir: &Path) -> std::io::Result<LogGuard> {
    std::fs::create_dir_all(logs_dir)?;

    let file_appender = tracing_appender::rolling::daily(logs_dir, "trace.ndjson");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("OXYRIS_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = fmt::layer().with_target(false).with_writer(std::io::stderr);
    let json_layer = fmt::layer()
        .json()
        .with_current_span(true)
        .with_writer(non_blocking);

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(json_layer)
        .init();

    Ok(LogGuard(guard))
}
