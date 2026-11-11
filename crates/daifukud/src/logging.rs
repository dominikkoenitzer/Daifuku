//! A daily log file. The daemon has no console, so the file is the only place
//! it can say anything.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::util::SubscriberInitExt;

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
    lines(writer, filter).try_init().ok()?;
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

/// The lines written to `writer`, as far as `filter` lets them through.
fn lines<W>(writer: W, filter: EnvFilter) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A writer into memory the test reads afterwards.
    #[derive(Clone, Default)]
    struct Memory(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Memory {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_log_line_holds_no_time_of_day() {
        let memory = Memory::default();
        let writer = memory.clone();
        tracing::subscriber::with_default(
            lines(move || writer.clone(), EnvFilter::new("info")),
            || tracing::info!(fleet = "agents", "opened a fleet"),
        );
        let text = String::from_utf8(memory.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("opened a fleet"), "{text}");
        assert!(!text.chars().any(|c| c.is_ascii_digit()), "{text}");
    }
}
