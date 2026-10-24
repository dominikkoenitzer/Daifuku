//! How much of its limits an account has used: the share of its 5-hour
//! window and of its weekly one, and when each starts over.
//!
//! Claude Code hands its status line command a JSON object on standard
//! input after each answer it gets. For a subscription that object carries
//! `rate_limits`: for both windows, the percentage used and when the window
//! resets, in seconds since 1970. `daifuku status-line` passes those numbers
//! on to the daemon, which keeps the latest for each account. Nothing here
//! asks the network or estimates: a number is as old as the last status line
//! that ran, and says so.

use serde::{Deserialize, Serialize};

use crate::config::{LimitWindow, LocalTime, StopAt};

/// How long the short window lasts, in seconds.
pub const FIVE_HOURS: u64 = 5 * 60 * 60;

/// How long the weekly window lasts, in seconds.
pub const SEVEN_DAYS: u64 = 7 * 24 * 60 * 60;

/// One window, as last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// The share used, in percent, 0 to 100.
    pub used: u8,
    /// When the window starts over, in seconds since 1970.
    pub resets_at: u64,
}

impl Window {
    /// Whether the window has started over since it was reported, which
    /// leaves its number meaning nothing.
    #[must_use]
    pub const fn has_reset(&self, now: u64) -> bool {
        now >= self.resets_at
    }

    /// What there is to tell of this window of kind `kind`, as an account
    /// with `rules` has it at `time`, against what was seen of it
    /// `before`: what is seen now and what changed. Nothing changed on
    /// first sight. A report that moves the reset by less than half the
    /// window's length is of the same window, so it never starts one over.
    #[must_use]
    pub fn watch(
        &self,
        kind: LimitWindow,
        rules: &[StopAt],
        time: LocalTime,
        before: Option<Seen>,
    ) -> (Seen, Option<Change>) {
        let reset = self.has_reset(time.now);
        let share = (!reset)
            .then(|| rules.iter().find(|r| r.holds(kind, self.resets_at, time)))
            .flatten()
            .map(|r| r.share);
        let mut seen = Seen {
            resets_at: self.resets_at,
            reset,
            share,
            stopped: share.is_some_and(|s| self.used >= s),
        };
        let Some(old) = before else {
            return (seen, None);
        };
        let length = match kind {
            LimitWindow::FiveHour => FIVE_HOURS,
            LimitWindow::SevenDay => SEVEN_DAYS,
        };
        if self.resets_at.abs_diff(old.resets_at) >= length / 2 {
            // An earlier window is a late report: what was seen stays.
            if self.resets_at < old.resets_at {
                return (old, None);
            }
            // A later one has started over since, unless that was told
            // already. Its stop is told on the next look.
            let started = !old.reset;
            seen.stopped = false;
            return (
                seen,
                started.then_some(Change::Reset {
                    used: (!reset).then_some(self.used),
                }),
            );
        }
        seen.reset |= old.reset;
        let change = if reset {
            (!old.reset).then_some(Change::Reset { used: None })
        } else if seen.stopped {
            (!old.stopped).then(|| Change::Limit {
                used: self.used,
                share: share.unwrap_or(100),
            })
        } else {
            // No stop counts as higher than any.
            let room = |s: Option<u8>| s.map_or(u16::MAX, u16::from);
            (room(share) > room(old.share)).then(|| Change::TopUp {
                used: self.used,
                share,
                resets_in_seconds: self.resets_at - time.now,
            })
        };
        (seen, change)
    }

    /// How far use is ahead of an even spread over a window `length`
    /// seconds long, in percentage points: `+12` is 12 more than the time
    /// gone by would allow, `-20` is 20 to spare. `None` once the window
    /// has reset.
    #[must_use]
    pub fn pace(&self, length: u64, now: u64) -> Option<i32> {
        if self.has_reset(now) || length == 0 {
            return None;
        }
        let left = (self.resets_at - now).min(length);
        let gone = u128::from(length - left) * 100 / u128::from(length);
        let gone = i32::try_from(gone).unwrap_or(100);
        Some(i32::from(self.used) - gone)
    }
}

/// What was last seen of a window, to tell what changed since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    /// When it starts over, as last reported.
    pub resets_at: u64,
    /// Whether it has started over since.
    pub reset: bool,
    /// The share its account's `stop_at` stops it at, `None` for no stop.
    pub share: Option<u8>,
    /// Whether it had used that share.
    pub stopped: bool,
}

/// A change to a window worth telling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// It used the share its account's `stop_at` stops at.
    Limit {
        /// Its share used.
        used: u8,
        /// The share it stops at.
        share: u8,
    },
    /// It started over.
    Reset {
        /// The new window's share used, once a report gave it.
        used: Option<u8>,
    },
    /// Its stop rose, or went, before it started over, as when a rule for
    /// its last hour takes over: it may take more than it could.
    TopUp {
        /// Its share used.
        used: u8,
        /// The share it stops at now, `None` for no stop.
        share: Option<u8>,
        /// How long until it starts over.
        resets_in_seconds: u64,
    },
}

/// Both windows; one Claude Code did not report is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// The 5-hour window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<Window>,
    /// The weekly window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<Window>,
}

impl Limits {
    /// Reads the limits out of what Claude Code hands a status line. An
    /// object without them, as from an API key or before the first answer,
    /// gives empty limits; only text that is not JSON is an error.
    ///
    /// # Errors
    ///
    /// When the input is not a JSON object.
    pub fn from_status_line(input: impl std::io::Read) -> Result<Self, serde_json::Error> {
        let input: StatusLine = serde_json::from_reader(input)?;
        let limits = input.rate_limits.unwrap_or_default();
        Ok(Self {
            five_hour: limits.five_hour.and_then(RawWindow::window),
            seven_day: limits.seven_day.and_then(RawWindow::window),
        })
    }

    /// True when neither window was reported.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.seven_day.is_none()
    }

    /// One line for a status line, such as `5h 48% (2h 14m left), week 35%
    /// (3d 4h left)`; empty when neither window was reported.
    #[must_use]
    pub fn line(&self, now: u64) -> String {
        [("5h", self.five_hour), ("week", self.seven_day)]
            .into_iter()
            .filter_map(|(name, w)| {
                let w = w?;
                Some(if w.has_reset(now) {
                    format!("{name} reset")
                } else {
                    format!("{name} {}% ({} left)", w.used, span(w.resets_at - now))
                })
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Whether either window, while it has not reset, has used `warn`
    /// percent or more.
    #[must_use]
    pub fn reaches(&self, warn: u8, now: u64) -> bool {
        [self.five_hour, self.seven_day]
            .into_iter()
            .flatten()
            .any(|w| !w.has_reset(now) && w.used >= warn)
    }

    /// Why an account's `stop_at` rules hand out no more work at `time`,
    /// such as `5h 80%, stops at 75%`: the first window whose first rule
    /// that holds it has used up. `None` while work may go on. A window that
    /// has reset since is used up by nothing.
    #[must_use]
    pub fn stopped_by(&self, rules: &[StopAt], time: LocalTime) -> Option<String> {
        [
            (LimitWindow::FiveHour, "5h", self.five_hour),
            (LimitWindow::SevenDay, "week", self.seven_day),
        ]
        .into_iter()
        .find_map(|(kind, name, w)| {
            let w = w.filter(|w| !w.has_reset(time.now))?;
            let rule = rules.iter().find(|r| r.holds(kind, w.resets_at, time))?;
            (w.used >= rule.share).then(|| format!("{name} {}%, stops at {}%", w.used, rule.share))
        })
    }
}

/// `3d 4h`, `2h 14m`, `9m`: how long until a window resets, to the minute.
#[must_use]
pub fn span(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    match minutes {
        0..60 => format!("{minutes}m"),
        60..1440 => format!("{}h {}m", minutes / 60, minutes % 60),
        _ => format!("{}d {}h", minutes / 1440, minutes % 1440 / 60),
    }
}

/// The few fields of the status line's input Daifuku reads.
#[derive(Deserialize)]
struct StatusLine {
    #[serde(default)]
    rate_limits: Option<RawLimits>,
}

#[derive(Default, Deserialize)]
struct RawLimits {
    #[serde(default)]
    five_hour: Option<RawWindow>,
    #[serde(default)]
    seven_day: Option<RawWindow>,
}

/// A window as Claude Code writes it: the share may have a fraction, and
/// either field may be missing or null.
#[derive(Deserialize)]
struct RawWindow {
    #[serde(default)]
    used_percentage: Option<f64>,
    #[serde(default)]
    resets_at: Option<f64>,
}

impl RawWindow {
    /// The window, when both fields are there and make sense. The share is
    /// rounded, so 76.5 % counts as 77 and reaches a warning at 77 a moment
    /// early rather than late.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "both are checked to be in range first"
    )]
    fn window(self) -> Option<Window> {
        let used = self.used_percentage?;
        let resets_at = self.resets_at?;
        if !used.is_finite() || !resets_at.is_finite() || resets_at <= 0.0 {
            return None;
        }
        Some(Window {
            used: used.round().clamp(0.0, 100.0) as u8,
            resets_at: resets_at.min(u64::MAX as f64) as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(json: &str) -> Limits {
        Limits::from_status_line(json.as_bytes()).unwrap()
    }

    #[test]
    fn the_status_line_s_rate_limits_are_read_and_the_rest_ignored() {
        let l = limits(
            r#"{"session_id":"s","model":{"id":"m"},"rate_limits":{
                "five_hour":{"used_percentage":48.4,"resets_at":1792548000},
                "seven_day":{"used_percentage":35,"resets_at":1792839600}}}"#,
        );
        assert_eq!(
            l.five_hour,
            Some(Window {
                used: 48,
                resets_at: 1_792_548_000
            })
        );
        assert_eq!(l.seven_day.unwrap().used, 35);
    }

    #[test]
    fn missing_or_odd_windows_are_left_out() {
        assert!(limits("{}").is_empty());
        assert!(limits(r#"{"rate_limits":null}"#).is_empty());
        let l = limits(
            r#"{"rate_limits":{"five_hour":{"used_percentage":null,"resets_at":1},
                "seven_day":{"used_percentage":250,"resets_at":5}}}"#,
        );
        assert_eq!(l.five_hour, None);
        assert_eq!(l.seven_day.unwrap().used, 100);
        assert!(
            limits(r#"{"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":-3}}}"#)
                .is_empty()
        );
        assert!(Limits::from_status_line("not json".as_bytes()).is_err());
    }

    #[test]
    fn a_share_is_rounded_so_a_warning_comes_early_not_late() {
        let l = limits(r#"{"rate_limits":{"five_hour":{"used_percentage":76.5,"resets_at":9}}}"#);
        assert_eq!(l.five_hour.unwrap().used, 77);
    }

    #[test]
    fn pace_is_use_less_the_share_of_the_window_gone_by() {
        let w = Window {
            used: 60,
            resets_at: 10_000 + FIVE_HOURS,
        };
        // An hour into five: 20 % gone by, 60 % used.
        assert_eq!(w.pace(FIVE_HOURS, 10_000 + 3600), Some(40));
        // At the very start nothing has gone by.
        assert_eq!(w.pace(FIVE_HOURS, 10_000), Some(60));
        // A reset further off than the window is long counts from its start.
        assert_eq!(w.pace(FIVE_HOURS, 0), Some(60));
        let light = Window { used: 10, ..w };
        assert_eq!(light.pace(FIVE_HOURS, 10_000 + 4 * 3600), Some(-70));
        assert_eq!(w.pace(FIVE_HOURS, 10_000 + FIVE_HOURS), None);
    }

    #[test]
    fn the_status_line_reads_both_windows_and_how_long_each_has_left() {
        let l = Limits {
            five_hour: Some(Window {
                used: 48,
                resets_at: 100 + 2 * 3600 + 14 * 60,
            }),
            seven_day: Some(Window {
                used: 35,
                resets_at: 100 + 3 * 86_400 + 4 * 3600,
            }),
        };
        assert_eq!(l.line(100), "5h 48% (2h 14m left), week 35% (3d 4h left)");
        assert_eq!(
            l.line(100 + 2 * 3600 + 14 * 60),
            "5h reset, week 35% (3d 1h left)"
        );
        assert_eq!(Limits::default().line(0), "");
        assert_eq!(span(1), "1m");
        assert_eq!(span(59 * 60 + 1), "1h 0m");
    }

    #[test]
    fn a_warning_holds_only_until_its_window_resets() {
        let l = Limits {
            five_hour: Some(Window {
                used: 80,
                resets_at: 100,
            }),
            seven_day: Some(Window {
                used: 30,
                resets_at: 1000,
            }),
        };
        assert!(l.reaches(77, 99));
        assert!(!l.reaches(81, 99));
        assert!(!l.reaches(77, 100), "the 5-hour window has reset");
        assert!(l.reaches(30, 100), "the weekly one has not");
        assert!(!Limits::default().reaches(0, 0));
    }

    #[test]
    fn the_first_rule_that_holds_for_a_window_decides_its_stop() {
        let rules: Vec<StopAt> = serde_json::from_str(
            r#"[{"share":90,"window":"five_hour","last_minutes":60},
                {"share":75,"window":"five_hour","days":"mon-fri","hours":"9-17"},
                {"share":95}]"#,
        )
        .unwrap();
        let now = 1_000_000;
        let five = |used, left| Limits {
            five_hour: Some(Window {
                used,
                resets_at: now + left,
            }),
            seven_day: Some(Window {
                used: 40,
                resets_at: now + 86_400,
            }),
        };
        let monday_ten = LocalTime {
            now,
            weekday: 0,
            minute: 600,
        };
        let saturday = LocalTime {
            weekday: 5,
            ..monday_ten
        };
        assert_eq!(
            five(80, 7200).stopped_by(&rules, monday_ten).as_deref(),
            Some("5h 80%, stops at 75%")
        );
        assert_eq!(five(74, 7200).stopped_by(&rules, monday_ten), None);
        assert_eq!(five(80, 7200).stopped_by(&rules, saturday), None);
        // In the last hour before the reset, the higher stop comes first.
        assert_eq!(five(80, 3600).stopped_by(&rules, monday_ten), None);
        assert!(five(90, 3600).stopped_by(&rules, monday_ten).is_some());
        let week = Limits {
            seven_day: Some(Window {
                used: 96,
                resets_at: now + 10,
            }),
            ..Limits::default()
        };
        assert_eq!(
            week.stopped_by(&rules, saturday).as_deref(),
            Some("week 96%, stops at 95%")
        );
        // Once it has reset, it stops nothing.
        let later = LocalTime {
            now: now + 10,
            ..saturday
        };
        assert_eq!(week.stopped_by(&rules, later), None);
        assert_eq!(five(100, 7200).stopped_by(&[], monday_ten), None);
    }

    /// Feeds each `(used, resets_at, now)` to a 5-hour window in turn and
    /// collects what changed at each.
    fn watched(rules: &[StopAt], steps: &[(u8, u64, u64)]) -> Vec<Option<Change>> {
        let mut seen = None;
        steps
            .iter()
            .map(|&(used, resets_at, now)| {
                let time = LocalTime {
                    now,
                    weekday: 0,
                    minute: 600,
                };
                let w = Window { used, resets_at };
                let (now, change) = w.watch(LimitWindow::FiveHour, rules, time, seen);
                seen = Some(now);
                change
            })
            .collect()
    }

    #[test]
    fn a_late_report_of_the_window_before_changes_nothing() {
        let end = 1_000_000;
        let late = (99, end - FIVE_HOURS, end - 7100);
        assert_eq!(
            watched(
                &stop_rules(),
                &[(60, end, end - 7200), late, (80, end, end - 7000)],
            ),
            vec![
                None,
                None,
                Some(Change::Limit {
                    used: 80,
                    share: 75
                })
            ]
        );
        assert_eq!(
            watched(
                &stop_rules(),
                &[
                    (60, end, end - 7200),
                    late,
                    (60, end, end + 1),
                    (3, end + FIVE_HOURS, end + 9),
                ],
            ),
            vec![None, None, Some(Change::Reset { used: None }), None]
        );
    }

    #[test]
    fn a_new_window_already_at_its_stop_is_told_both() {
        let end = 1_000_000;
        let new = (80, end + FIVE_HOURS, end + 7200);
        assert_eq!(
            watched(&stop_rules(), &[(60, end, end - 10), new, new, new]),
            vec![
                None,
                Some(Change::Reset { used: Some(80) }),
                Some(Change::Limit {
                    used: 80,
                    share: 75
                }),
                None,
            ]
        );
    }

    fn stop_rules() -> Vec<StopAt> {
        serde_json::from_str(r#"[{"share":91,"last_minutes":60},{"share":75}]"#).unwrap()
    }

    #[test]
    fn a_window_is_told_once_when_it_reaches_its_stop() {
        let end = 1_000_000;
        let changes = watched(
            &stop_rules(),
            &[
                (70, end, end - 7200),
                (76, end, end - 7100),
                (80, end, end - 7000),
            ],
        );
        assert_eq!(
            changes,
            [
                None,
                Some(Change::Limit {
                    used: 76,
                    share: 75
                }),
                None
            ]
        );
        // Without rules nothing stops.
        assert!(
            watched(&[], &[(70, end, end - 7200), (100, end, end - 7100)])
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn a_window_is_told_once_when_it_starts_over() {
        let end = 1_000_000;
        // Seen to pass its reset, then reported anew.
        let changes = watched(
            &stop_rules(),
            &[
                (60, end, end - 10),
                (60, end, end),
                (60, end, end + 2),
                (3, end + FIVE_HOURS, end + 9),
            ],
        );
        assert_eq!(
            changes,
            [None, Some(Change::Reset { used: None }), None, None]
        );
        // Reported anew before the time was seen to pass.
        let changes = watched(
            &stop_rules(),
            &[(60, end, end - 10), (3, end + FIVE_HOURS, end + 9)],
        );
        assert_eq!(changes, [None, Some(Change::Reset { used: Some(3) })]);
    }

    #[test]
    fn a_reset_that_moves_a_little_is_the_same_window() {
        let end = 1_000_000;
        let changes = watched(
            &stop_rules(),
            &[
                (60, end, end - 7200),
                (61, end + 5, end - 7100),
                (62, end - 5, end - 7000),
                // A late report of the window before is not a reset.
                (99, end - FIVE_HOURS, end - 6900),
            ],
        );
        assert!(changes.iter().all(Option::is_none), "{changes:?}");
    }

    #[test]
    fn a_stop_that_rises_before_the_reset_is_a_top_up() {
        let end = 1_000_000;
        let changes = watched(
            &stop_rules(),
            &[
                // Stopped at 75 until the last hour, then 91 holds.
                (80, end, end - 3700),
                (80, end, end - 3500),
                (85, end, end - 3000),
                (92, end, end - 2000),
            ],
        );
        assert_eq!(
            changes,
            [
                None,
                Some(Change::TopUp {
                    used: 80,
                    share: Some(91),
                    resets_in_seconds: 3500
                }),
                None,
                Some(Change::Limit {
                    used: 92,
                    share: 91
                }),
            ]
        );
        // A stop that goes is a top-up too; one that falls is not.
        let hours: Vec<StopAt> = serde_json::from_str(r#"[{"share":75,"hours":"9-17"}]"#).unwrap();
        let mut seen = None;
        let changes: Vec<_> = [600, 1020, 600]
            .into_iter()
            .map(|minute| {
                let time = LocalTime {
                    now: end - 7200,
                    weekday: 0,
                    minute,
                };
                let w = Window {
                    used: 50,
                    resets_at: end,
                };
                let (now, change) = w.watch(LimitWindow::FiveHour, &hours, time, seen);
                seen = Some(now);
                change
            })
            .collect();
        assert_eq!(
            changes,
            [
                None,
                Some(Change::TopUp {
                    used: 50,
                    share: None,
                    resets_in_seconds: 7200
                }),
                None
            ]
        );
    }
}
