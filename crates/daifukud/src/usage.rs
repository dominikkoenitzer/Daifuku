//! What the daemon knows of each account's limits: the latest numbers a
//! status line reported for it, and which terminal runs on which account.
//!
//! An account is its Claude Code folder, as the status line found it. Its
//! name comes from the config each time it is asked for, so renaming an
//! account or adding one for a folder already reported needs no new report.

use std::path::{Path, PathBuf};

use daifuku_core::config::{Config, folder_key};
use daifuku_core::protocol::{AccountUsage, UsageReport, WindowUsage};
use daifuku_core::usage::{FIVE_HOURS, Limits, SEVEN_DAYS, Window};
use serde::{Deserialize, Serialize};

/// The most accounts kept: more than anyone signs in with, few enough that
/// a stream of made-up folders cannot grow the daemon.
const MAX_ACCOUNTS: usize = 16;

/// The most terminals whose account is kept, the most recent ones.
const MAX_WINDOWS: usize = 256;

/// The longest folder taken, in characters.
const MAX_FOLDER: usize = 1024;

/// One account's latest numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    /// Its Claude Code folder, as the status line wrote it.
    folder: String,
    /// When they were reported, in seconds since 1970.
    at: u64,
    limits: Limits,
}

/// The latest numbers of every account, and the account of every terminal
/// a status line reported from.
#[derive(Debug, Default)]
pub struct Book {
    /// Oldest report first.
    entries: Vec<Entry>,
    /// Terminal windows and their account's folder, least recent first.
    windows: Vec<(u64, String)>,
    /// Where the numbers are kept across a restart, `None` when they are
    /// not, as for the daemon's other state.
    pub file: Option<PathBuf>,
}

impl Book {
    /// Takes in one report, `window` being the terminal it came from as the
    /// daemon checked it. True when an account's numbers changed, and so
    /// perhaps its warning.
    pub fn note(&mut self, report: &UsageReport, window: Option<u64>, now: u64) -> bool {
        let folder = report.folder.trim();
        if folder.is_empty() || folder.chars().count() > MAX_FOLDER {
            return false;
        }
        if let Some(w) = window {
            self.windows.retain(|(x, _)| *x != w);
            if self.windows.len() >= MAX_WINDOWS {
                self.windows.remove(0);
            }
            self.windows.push((w, folder.to_owned()));
        }
        if report.usage.is_empty() {
            return false;
        }
        let key = folder_key(Path::new(folder));
        let old = self
            .entries
            .iter()
            .position(|e| folder_key(Path::new(&e.folder)) == key)
            .map(|i| self.entries.remove(i));
        if old.is_none() && self.entries.len() >= MAX_ACCOUNTS {
            self.entries.remove(0);
        }
        // A window the report left out keeps its last number until it
        // resets: Claude Code reports each one only once it knows it.
        let limits = Limits {
            five_hour: report
                .usage
                .five_hour
                .or_else(|| old.as_ref().and_then(|e| e.limits.five_hour)),
            seven_day: report
                .usage
                .seven_day
                .or_else(|| old.as_ref().and_then(|e| e.limits.seven_day)),
        };
        let changed = old.as_ref().is_none_or(|e| e.limits != limits);
        self.entries.push(Entry {
            folder: folder.to_owned(),
            at: now,
            limits,
        });
        changed
    }

    /// The folder of the account terminal `w` last reported from.
    pub fn folder_of(&self, w: u64) -> Option<&str> {
        self.windows
            .iter()
            .find(|(x, _)| *x == w)
            .map(|(_, f)| f.as_str())
    }

    /// Whether the account in `folder` has reached the `warn` the config
    /// gives it.
    pub fn warning(&self, config: &Config, folder: &Path, now: u64) -> bool {
        let Some(warn) = config.account_in(folder).and_then(|(_, a)| a.warn) else {
            return false;
        };
        let key = folder_key(folder);
        self.entries
            .iter()
            .find(|e| folder_key(Path::new(&e.folder)) == key)
            .is_some_and(|e| e.limits.reaches(warn, now))
    }

    /// Every account, most recently reported first, for `daifuku status`.
    pub fn status(&self, config: &Config, now: u64) -> Vec<AccountUsage> {
        self.entries
            .iter()
            .rev()
            .map(|e| {
                let folder = Path::new(&e.folder);
                let account = config.account_in(folder);
                AccountUsage {
                    account: account.map(|(n, _)| n.to_owned()),
                    folder: e.folder.clone(),
                    age_seconds: now.saturating_sub(e.at),
                    five_hour: e.limits.five_hour.map(|w| window(w, FIVE_HOURS, now)),
                    seven_day: e.limits.seven_day.map(|w| window(w, SEVEN_DAYS, now)),
                    warn: account.and_then(|(_, a)| a.warn),
                    warning: self.warning(config, folder, now),
                }
            })
            .collect()
    }

    /// Reads [`Book::file`]. An account whose windows have all reset since
    /// has nothing left worth showing and is dropped.
    pub fn load(&mut self, now: u64) {
        let Some(file) = &self.file else { return };
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        let Ok(mut entries) = serde_json::from_str::<Vec<Entry>>(&text) else {
            tracing::warn!(path = %file.display(), "ignored an unreadable usage file");
            return;
        };
        entries.retain(|e| {
            [e.limits.five_hour, e.limits.seven_day]
                .into_iter()
                .flatten()
                .any(|w| !w.has_reset(now))
        });
        entries.truncate(MAX_ACCOUNTS);
        self.entries = entries;
    }

    /// Writes [`Book::file`].
    pub fn save(&self) {
        let Some(file) = &self.file else { return };
        let written = serde_json::to_string_pretty(&self.entries)
            .map_err(std::io::Error::other)
            .and_then(|text| crate::daemon::replace_file(file, &text));
        if let Err(e) = written {
            tracing::warn!(path = %file.display(), error = %e, "could not save the usage");
        }
    }
}

fn window(w: Window, length: u64, now: u64) -> WindowUsage {
    WindowUsage {
        used: w.used,
        resets_at: w.resets_at,
        resets_in_seconds: (!w.has_reset(now)).then(|| w.resets_at - now),
        pace: w.pace(length, now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn report(folder: &str, five: Option<u8>, week: Option<u8>) -> UsageReport {
        UsageReport {
            usage: Limits {
                five_hour: five.map(|used| Window {
                    used,
                    resets_at: NOW + 3600,
                }),
                seven_day: week.map(|used| Window {
                    used,
                    resets_at: NOW + 86_400,
                }),
            },
            folder: folder.to_owned(),
            window: None,
        }
    }

    fn config() -> Config {
        Config::from_json(
            r#"{"accounts":{"work":{"claude":"C:\\Users\\me\\.claude-work","warn":77}}}"#,
        )
        .unwrap()
    }

    #[test]
    fn the_latest_report_of_an_account_wins_in_any_case_of_its_folder() {
        let mut b = Book::default();
        assert!(b.note(
            &report(r"C:\Users\me\.claude-work", Some(40), Some(20)),
            Some(7),
            NOW
        ));
        assert!(!b.note(
            &report(r"c:\users\me\.claude-work\", Some(40), Some(20)),
            None,
            NOW + 5
        ));
        assert!(b.note(
            &report(r"C:\Users\me\.claude-work", Some(80), None),
            None,
            NOW + 9
        ));
        let s = b.status(&config(), NOW + 10);
        assert_eq!(s.len(), 1);
        let work = &s[0];
        assert_eq!(work.account.as_deref(), Some("work"));
        assert_eq!(work.age_seconds, 1);
        assert_eq!(work.five_hour.unwrap().used, 80);
        // The weekly number the last report left out stays.
        assert_eq!(work.seven_day.unwrap().used, 20);
        assert!(work.warning);
        assert_eq!(b.folder_of(7), Some(r"C:\Users\me\.claude-work"));
    }

    #[test]
    fn an_account_without_warn_or_name_never_warns() {
        let mut b = Book::default();
        b.note(
            &report(r"C:\Users\me\.claude", Some(99), Some(99)),
            None,
            NOW,
        );
        let s = b.status(&config(), NOW);
        assert_eq!(
            (s[0].account.as_deref(), s[0].warn, s[0].warning),
            (None, None, false)
        );
    }

    #[test]
    fn a_warning_ends_when_its_window_resets() {
        let mut b = Book::default();
        b.note(
            &report(r"C:\Users\me\.claude-work", Some(90), Some(10)),
            None,
            NOW,
        );
        let folder = Path::new(r"C:\Users\me\.claude-work");
        assert!(b.warning(&config(), folder, NOW + 3599));
        assert!(!b.warning(&config(), folder, NOW + 3600));
        let five = b.status(&config(), NOW + 3600)[0].five_hour.unwrap();
        assert_eq!((five.resets_in_seconds, five.pace), (None, None));
    }

    #[test]
    fn a_report_with_no_numbers_still_says_which_account_a_terminal_runs_on() {
        let mut b = Book::default();
        let mut r = report(r"C:\x", None, None);
        r.window = Some(3);
        assert!(!b.note(&r, Some(3), NOW));
        assert_eq!(b.folder_of(3), Some(r"C:\x"));
        assert!(b.status(&config(), NOW).is_empty());
    }

    #[test]
    fn made_up_folders_cannot_grow_the_book_without_end() {
        let mut b = Book::default();
        for i in 0..100 {
            b.note(&report(&format!(r"C:\f{i}"), Some(1), None), Some(i), NOW);
        }
        assert_eq!(b.status(&config(), NOW).len(), MAX_ACCOUNTS);
        assert!(b.folder_of(99).is_some());
        assert!(!b.note(&report("  ", Some(1), None), None, NOW));
    }

    #[test]
    fn the_book_survives_a_restart_without_what_has_reset() {
        let dir = std::env::temp_dir().join(format!("daifuku-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("usage.json");
        let mut b = Book {
            file: Some(file.clone()),
            ..Book::default()
        };
        b.note(&report(r"C:\a", Some(50), None), None, NOW);
        b.note(&report(r"C:\b", Some(5), Some(5)), None, NOW);
        b.save();
        let mut again = Book {
            file: Some(file),
            ..Book::default()
        };
        // An hour on, the first account's only window has reset.
        again.load(NOW + 3600);
        let s = again.status(&config(), NOW + 3600);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].folder, r"C:\b");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
