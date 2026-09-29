//! Choosing a monitor for a fleet, from a list the daemon read off Win32.

use serde::{Deserialize, Serialize};

use crate::config::MonitorPick;
use crate::geometry::Rect;

/// One attached monitor, in physical pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// The device name, `\\.\DISPLAY2`.
    pub device: String,
    /// The whole screen.
    pub bounds: Rect,
    /// The screen minus the taskbar: where terminals go.
    pub work: Rect,
    /// Effective DPI, 96 at 100 %.
    pub dpi: u32,
    /// The primary monitor.
    pub primary: bool,
}

/// The monitor a fleet should open on.
///
/// Every pick falls back to the primary monitor, and a machine with no
/// primary (which Windows does not produce, but a list from a test can) to
/// the first one, so a fleet always has somewhere to go. `None` only for an
/// empty list.
#[must_use]
pub fn pick<'a>(
    monitors: &'a [MonitorInfo],
    choice: &MonitorPick,
    cursor: (i32, i32),
) -> Option<&'a MonitorInfo> {
    let primary = || {
        monitors
            .iter()
            .find(|m| m.primary)
            .or_else(|| monitors.first())
    };
    match choice {
        MonitorPick::Portrait => monitors
            .iter()
            .find(|m| m.bounds.is_portrait())
            .or_else(primary),
        MonitorPick::Landscape => monitors
            .iter()
            .filter(|m| !m.bounds.is_portrait())
            .max_by_key(|m| (m.primary, std::cmp::Reverse(position(monitors, m))))
            .or_else(primary),
        MonitorPick::Primary => primary(),
        MonitorPick::Secondary => monitors.iter().find(|m| !m.primary).or_else(primary),
        MonitorPick::Cursor => monitors
            .iter()
            .find(|m| m.bounds.contains(cursor.0, cursor.1))
            .or_else(primary),
        MonitorPick::Device(name) => monitors
            .iter()
            .find(|m| m.device.eq_ignore_ascii_case(name))
            .or_else(primary),
    }
}

fn position(monitors: &[MonitorInfo], m: &MonitorInfo) -> usize {
    monitors
        .iter()
        .position(|o| std::ptr::eq(o, m))
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(device: &str, bounds: Rect, primary: bool) -> MonitorInfo {
        MonitorInfo {
            device: device.into(),
            bounds,
            work: bounds,
            dpi: 96,
            primary,
        }
    }

    /// The machine Daifuku was built on: a portrait screen listed first, the
    /// 4K primary second.
    fn desk() -> Vec<MonitorInfo> {
        vec![
            mon(r"\\.\DISPLAY2", Rect::new(3840, 120, 4920, 2040), false),
            mon(r"\\.\DISPLAY1", Rect::new(0, 0, 3840, 2160), true),
        ]
    }

    fn device(m: Option<&MonitorInfo>) -> &str {
        m.map_or("none", |m| m.device.as_str())
    }

    #[test]
    fn every_pick_on_the_build_machine() {
        let d = desk();
        assert_eq!(
            device(pick(&d, &MonitorPick::Portrait, (0, 0))),
            r"\\.\DISPLAY2"
        );
        assert_eq!(
            device(pick(&d, &MonitorPick::Landscape, (0, 0))),
            r"\\.\DISPLAY1"
        );
        assert_eq!(
            device(pick(&d, &MonitorPick::Primary, (0, 0))),
            r"\\.\DISPLAY1"
        );
        assert_eq!(
            device(pick(&d, &MonitorPick::Secondary, (0, 0))),
            r"\\.\DISPLAY2"
        );
        assert_eq!(
            device(pick(&d, &MonitorPick::Cursor, (4000, 500))),
            r"\\.\DISPLAY2"
        );
        assert_eq!(
            device(pick(&d, &MonitorPick::Cursor, (10, 10))),
            r"\\.\DISPLAY1"
        );
        assert_eq!(
            device(pick(
                &d,
                &MonitorPick::Device(r"\\.\display2".into()),
                (0, 0)
            )),
            r"\\.\DISPLAY2"
        );
    }

    #[test]
    fn a_missing_monitor_falls_back_to_the_primary() {
        let single = vec![mon(r"\\.\DISPLAY1", Rect::new(0, 0, 2560, 1440), true)];
        for choice in [
            MonitorPick::Portrait,
            MonitorPick::Secondary,
            MonitorPick::Device(r"\\.\DISPLAY9".into()),
        ] {
            assert_eq!(
                device(pick(&single, &choice, (0, 0))),
                r"\\.\DISPLAY1",
                "{choice}"
            );
        }
        assert_eq!(
            device(pick(&single, &MonitorPick::Cursor, (-5000, -5000))),
            r"\\.\DISPLAY1"
        );
    }

    #[test]
    fn landscape_prefers_the_primary_then_the_first_listed() {
        let d = vec![
            mon("a", Rect::new(-1920, 0, 0, 1080), false),
            mon("b", Rect::new(0, 0, 2560, 1440), true),
            mon("c", Rect::new(2560, 0, 4480, 1080), false),
        ];
        assert_eq!(device(pick(&d, &MonitorPick::Landscape, (0, 0))), "b");
        let no_primary: Vec<_> = d
            .into_iter()
            .map(|m| MonitorInfo {
                primary: false,
                ..m
            })
            .collect();
        assert_eq!(
            device(pick(&no_primary, &MonitorPick::Landscape, (0, 0))),
            "a"
        );
    }

    #[test]
    fn an_empty_list_has_nowhere_to_go() {
        assert!(pick(&[], &MonitorPick::Primary, (0, 0)).is_none());
    }
}
