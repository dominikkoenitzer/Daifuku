//! What the daemon knows of each account's limits: the latest numbers a
//! status line reported for it, and which terminal runs on which account.
//!
//! An account is its Claude Code folder, as the status line found it. Its
//! name comes from the config each time it is asked for, so renaming an
//! account or adding one for a folder already reported needs no new report.

use std::path::{Path, PathBuf};

use daifuku_core::config::{Config, LimitWindow, LocalTime, folder_key};
use daifuku_core::protocol::{AccountUsage, FleetEvent, UsageReport, WindowUsage};
use daifuku_core::usage::{Change, FIVE_HOURS, Limits, SEVEN_DAYS, Seen, Window};
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
    /// What [`Book::watch`] last saw of each account's windows.
    seen: Vec<(Vec<String>, LimitWindow, Seen)>,
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
            five_hour: newer(
                report.usage.five_hour,
                old.as_ref().and_then(|e| e.limits.five_hour),
                FIVE_HOURS,
            ),
            seven_day: newer(
                report.usage.seven_day,
                old.as_ref().and_then(|e| e.limits.seven_day),
                SEVEN_DAYS,
            ),
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

    /// The latest numbers of the account in `folder`.
    pub fn limits(&self, folder: &Path) -> Option<Limits> {
        let key = folder_key(folder);
        self.entries
            .iter()
            .find(|e| folder_key(Path::new(&e.folder)) == key)
            .map(|e| e.limits)
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

    /// What changed in each account's windows since the last look: one
    /// reached its stop, started over or may take more. Nothing on the
    /// first look at a window, as after a restart.
    pub fn watch(&mut self, config: &Config, time: LocalTime) -> Vec<FleetEvent> {
        let mut seen = Vec::new();
        let mut events = Vec::new();
        for e in &self.entries {
            let folder = Path::new(&e.folder);
            let key = folder_key(folder);
            let account = config.account_in(folder);
            let rules = account.map_or(&[][..], |(_, a)| a.stop_at.as_slice());
            for (kind, w) in [
                (LimitWindow::FiveHour, e.limits.five_hour),
                (LimitWindow::SevenDay, e.limits.seven_day),
            ] {
                let Some(w) = w else { continue };
                let before = self
                    .seen
                    .iter()
                    .find(|(k, x, _)| *k == key && *x == kind)
                    .map(|(_, _, s)| *s);
                let (now, change) = w.watch(kind, rules, time, before);
                seen.push((key.clone(), kind, now));
                let account = account.map(|(n, _)| n.to_owned());
                let folder = e.folder.clone();
                events.extend(change.map(|c| match c {
                    Change::Limit { used, share } => FleetEvent::Limit {
                        account,
                        folder,
                        limit: kind,
                        used,
                        share,
                    },
                    Change::Reset { used } => FleetEvent::Reset {
                        account,
                        folder,
                        limit: kind,
                        used,
                    },
                    Change::TopUp {
                        used,
                        share,
                        resets_in_seconds,
                    } => FleetEvent::TopUp {
                        account,
                        folder,
                        limit: kind,
                        used,
                        share,
                        resets_in_seconds,
                    },
                }));
            }
        }
        self.seen = seen;
        events
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

/// What a report says of a window `length` seconds long, else what was
/// known: a terminal that still shows the window before the one known keeps
/// the newer numbers.
fn newer(reported: Option<Window>, known: Option<Window>, length: u64) -> Option<Window> {
    match (reported, known) {
        (Some(r), Some(k)) if k.resets_at > r.resets_at.saturating_add(length / 2) => Some(k),
        (r, k) => r.or(k),
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
    fn a_report_from_before_the_reset_keeps_the_newer_window() {
        let mut b = Book::default();
        let folder = r"C:\Users\me\.claude-work";
        b.note(&report(folder, Some(5), Some(30)), None, NOW);
        let mut late = report(folder, Some(90), Some(29));
        late.usage.five_hour.as_mut().unwrap().resets_at -= FIVE_HOURS;
        late.usage.seven_day.as_mut().unwrap().resets_at -= 60;
        assert!(b.note(&late, None, NOW + 5));
        let s = b.status(&config(), NOW + 5);
        assert_eq!(s[0].five_hour.unwrap().used, 5);
        // The same week a little later is still the latest report.
        assert_eq!(s[0].seven_day.unwrap().used, 29);
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
    fn each_account_s_windows_are_watched_by_its_own_rules() {
        let config = Config::from_json(
            r#"{"accounts":{"work":{"claude":"C:\\Users\\me\\.claude-work",
                "stop_at":[{"share":75}]}}}"#,
        )
        .unwrap();
        let at = |now| LocalTime {
            now,
            weekday: 0,
            minute: 600,
        };
        let mut b = Book::default();
        b.note(
            &report(r"C:\Users\me\.claude-work", Some(70), Some(20)),
            None,
            NOW,
        );
        b.note(&report(r"C:\Users\me\.claude", Some(90), None), None, NOW);
        assert!(
            b.watch(&config, at(NOW)).is_empty(),
            "nothing on first sight"
        );
        b.note(
            &report(r"c:\users\me\.claude-work", Some(80), Some(21)),
            None,
            NOW + 1,
        );
        assert_eq!(
            b.watch(&config, at(NOW + 1)),
            [FleetEvent::Limit {
                account: Some("work".into()),
                folder: r"c:\users\me\.claude-work".into(),
                limit: LimitWindow::FiveHour,
                used: 80,
                share: 75,
            }]
        );
        assert!(b.watch(&config, at(NOW + 2)).is_empty(), "told once");
        // An hour on both 5-hour windows have started over; the account
        // without rules has no stop to reach, but its reset is told.
        let mut events = b.watch(&config, at(NOW + 3600));
        events.sort_by_key(|e| format!("{e:?}"));
        assert_eq!(
            events,
            [
                FleetEvent::Reset {
                    account: None,
                    folder: r"C:\Users\me\.claude".into(),
                    limit: LimitWindow::FiveHour,
                    used: None,
                },
                FleetEvent::Reset {
                    account: Some("work".into()),
                    folder: r"c:\users\me\.claude-work".into(),
                    limit: LimitWindow::FiveHour,
                    used: None,
                },
            ]
        );
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
