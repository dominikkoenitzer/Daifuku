//! A daily log file. The daemon has no console, so the file is the only place
//! it can say anything.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Starts logging into `dir`, at `info` unless `RUST_LOG` says otherwise.
/// Keep the guard alive for as long as the daemon runs: dropping it flushes
/// and stops the writer.
pub fn init(dir: Option<&Path>) -> Option<WorkerGuard> {
    let dir = dir?;
    std::fs::create_dir_all(dir).ok()?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("daifukud")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init()
        .ok()?;
    // With no console, a panic would otherwise leave no trace at all.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(
            panic = %info,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "daifukud panicked"
        );
        previous(info);
    }));
    Some(guard)
}
