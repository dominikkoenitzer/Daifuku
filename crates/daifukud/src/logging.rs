//! The log file. The daemon has no console, so the file is the only place it
//! can say anything. Its lines hold no time: the newest is the last one. Its
//! times on disk hold the day only.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::util::SubscriberInitExt;

/// How large the log grows before it starts over, in bytes.
const MAX_BYTES: u64 = 1_048_576;

/// How many log files are kept: `daifukud.log` and the older ones,
/// `daifukud.1.log` the newest of those.
const KEPT: usize = 7;

/// Starts logging into `dir`, at `info` unless `RUST_LOG` says otherwise.
/// Keep the guard alive for as long as the daemon runs: dropping it flushes
/// and stops the writer.
pub fn init(dir: Option<&Path>) -> Option<WorkerGuard> {
    let dir = dir?;
    std::fs::create_dir_all(dir).ok()?;
    remove_dated(dir);
    let file = Rolling::open(dir, MAX_BYTES, KEPT).ok()?;
    let (writer, guard) = tracing_appender::non_blocking(file);
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
        .without_time()
        .finish()
}

/// The log file `n` in `dir`: 0 the one written now, then older ones.
fn log_path(dir: &Path, n: usize) -> PathBuf {
    if n == 0 {
        dir.join("daifukud.log")
    } else {
        dir.join(format!("daifukud.{n}.log"))
    }
}

/// Removes the logs earlier versions wrote, a file a day named by its date.
fn remove_dated(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let dated = name
            .to_str()
            .and_then(|n| n.strip_prefix("daifukud."))
            .and_then(|n| n.strip_suffix(".log"))
            .is_some_and(|d| {
                d.len() == 10
                    && d.bytes().enumerate().all(|(i, b)| {
                        if i == 4 || i == 7 {
                            b == b'-'
                        } else {
                            b.is_ascii_digit()
                        }
                    })
            });
        if dated {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A log file that starts over once it holds `max` bytes, keeping `kept`
/// files in all, the oldest dropped.
struct Rolling {
    dir: PathBuf,
    file: File,
    written: u64,
    max: u64,
    kept: usize,
    /// The start of the day the file's times hold, `None` before they do.
    day: Option<i64>,
}

impl Rolling {
    /// Opens the log in `dir` to write on at its end.
    fn open(dir: &Path, max: u64, kept: usize) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(dir, 0))?;
        let written = file.metadata()?.len();
        let mut log = Self {
            dir: dir.to_owned(),
            file,
            written,
            max,
            kept,
            day: None,
        };
        log.hold_day();
        // The older logs an earlier version wrote keep their moments until
        // a roll moves them, so they get the day now.
        for n in 1..kept {
            let _ = daifuku_win::day::keep_day_only(&log_path(dir, n));
        }
        Ok(log)
    }

    /// Sets the times of the file written now and of the folder to the
    /// start of today, and holds the file's there while it is written, so a
    /// line costs no call to keep them.
    fn hold_day(&mut self) {
        self.day = daifuku_win::day::start_of_today().ok();
        let _ = daifuku_win::day::hold_day_only(&self.file);
        let _ = daifuku_win::day::keep_day_only(&self.dir);
    }

    /// Moves every log one older, the oldest out, and starts a new one. A
    /// move that fails leaves that file where it is; the log goes on.
    fn roll(&mut self) {
        let _ = std::fs::remove_file(log_path(&self.dir, self.kept - 1));
        for n in (0..self.kept - 1).rev() {
            let _ = std::fs::rename(log_path(&self.dir, n), log_path(&self.dir, n + 1));
        }
        // A move changes a file's times as well as its name.
        for n in 1..self.kept {
            let _ = daifuku_win::day::keep_day_only(&log_path(&self.dir, n));
        }
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(&self.dir, 0))
        {
            self.file = file;
        }
        self.written = 0;
        self.hold_day();
    }
}

impl Write for Rolling {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max {
            self.roll();
        } else if daifuku_win::day::start_of_today().ok() != self.day {
            // A daemon that runs past midnight moves its log to the new day.
            self.hold_day();
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
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

    /// A new empty folder for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daifuku-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(dir: &Path, n: usize) -> Option<String> {
        std::fs::read_to_string(log_path(dir, n)).ok()
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

    #[test]
    fn a_full_log_starts_over_and_only_the_newest_are_kept() {
        let dir = scratch("roll");
        let mut log = Rolling::open(&dir, 8, 3).unwrap();
        for line in ["a", "b", "c", "d", "e", "f", "g"] {
            log.write_all(format!("{line}{line}{line}\n").as_bytes())
                .unwrap();
        }
        drop(log);
        assert_eq!(read(&dir, 0).as_deref(), Some("ggg\n"));
        assert_eq!(read(&dir, 1).as_deref(), Some("eee\nfff\n"));
        assert_eq!(read(&dir, 2).as_deref(), Some("ccc\nddd\n"));
        assert_eq!(read(&dir, 3), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each of `paths` whose times hold more than the start of today.
    fn finer_than_the_day(paths: &[PathBuf]) -> Vec<String> {
        use daifuku_win::day::{Times, start_of_today, times};
        let day = start_of_today().unwrap();
        let only_day = Times {
            created: day,
            accessed: day,
            written: day,
            changed: day,
        };
        paths
            .iter()
            .filter_map(|path| {
                let kept = times(path).unwrap();
                (kept != only_day).then(|| format!("{}: {kept:?} for {day}", path.display()))
            })
            .collect()
    }

    #[test]
    fn the_logs_and_their_folder_hold_only_the_day_in_their_times() {
        let dir = scratch("day");
        // What the last daemon left, with the moment it wrote it.
        std::fs::write(log_path(&dir, 0), "old\n").unwrap();
        let mut log = Rolling::open(&dir, 8, 3).unwrap();
        for line in ["a", "b", "c", "d"] {
            log.write_all(format!("{line}{line}{line}\n").as_bytes())
                .unwrap();
            log.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(log);
        let mut paths: Vec<PathBuf> = [0, 1, 2].map(|n| log_path(&dir, n)).into();
        paths.push(dir.clone());
        let finer = finer_than_the_day(&paths);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(finer, Vec::<String>::new());
    }

    #[test]
    fn the_older_logs_an_earlier_daemon_left_hold_only_the_day_too() {
        let dir = scratch("dayold");
        for n in [0, 1, 2] {
            std::fs::write(log_path(&dir, n), "old\n").unwrap();
        }
        let mut log = Rolling::open(&dir, 1 << 20, 3).unwrap();
        log.write_all(b"new\n").unwrap();
        drop(log);
        let finer = finer_than_the_day(&[1, 2].map(|n| log_path(&dir, n)));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(finer, Vec::<String>::new());
    }

    #[test]
    fn the_log_goes_on_where_the_last_daemon_left_it() {
        let dir = scratch("reopen");
        std::fs::write(log_path(&dir, 0), "old\nold\n").unwrap();
        let mut log = Rolling::open(&dir, 12, 3).unwrap();
        log.write_all(b"new\n").unwrap();
        assert_eq!(read(&dir, 0).as_deref(), Some("old\nold\nnew\n"));
        log.write_all(b"one\n").unwrap();
        drop(log);
        assert_eq!(read(&dir, 0).as_deref(), Some("one\n"));
        assert_eq!(read(&dir, 1).as_deref(), Some("old\nold\nnew\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_longer_than_the_limit_leaves_no_empty_log_behind() {
        let dir = scratch("long");
        let mut log = Rolling::open(&dir, 2, 3).unwrap();
        log.write_all(b"a long line\n").unwrap();
        drop(log);
        assert_eq!(read(&dir, 0).as_deref(), Some("a long line\n"));
        assert_eq!(read(&dir, 1), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_logs_named_by_their_date_go_and_nothing_else() {
        let dir = scratch("dated");
        let names = [
            "daifukud.2026-11-10.log",
            "daifukud.log",
            "daifukud.1.log",
            "daifukud.2026-11-1x.log",
            "daifukud.2026_11_10.log",
            "install.log",
            "other.2026-11-10.log",
        ];
        for name in names {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        remove_dated(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "daifukud.1.log",
                "daifukud.2026-11-1x.log",
                "daifukud.2026_11_10.log",
                "daifukud.log",
                "install.log",
                "other.2026-11-10.log",
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
