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
}
