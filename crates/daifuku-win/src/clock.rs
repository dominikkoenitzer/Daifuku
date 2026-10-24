//! A clock for putting events in order that setting the time does not move,
//! and the local time of day.

use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::WindowsProgramming::QueryInterruptTimePrecise;

/// The interrupt time: 100 ns ticks since Windows started, the same in every
/// process. Unlike the wall clock it never steps back, whether the time is
/// set by hand, by a time zone or by a sync with a time server.
#[must_use]
pub fn ticks() -> u64 {
    // SAFETY: no arguments; only reads the system's interrupt time.
    unsafe { QueryInterruptTimePrecise() }
}

/// The local day of the week, Monday being 0, and the minute of the day, as
/// the clock in the taskbar shows them.
#[must_use]
pub fn local_day_and_minute() -> (u8, u16) {
    // SAFETY: no arguments; only reads the clock.
    let t = unsafe { GetLocalTime() };
    let day = u8::try_from((t.wDayOfWeek + 6) % 7).unwrap_or(0);
    (day, t.wHour * 60 + t.wMinute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_only_go_forward() {
        let first = ticks();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = ticks();
        assert!(first > 0);
        assert!(second > first, "{first} then {second}");
        // 5 ms is 50 000 ticks; a sleep may run long, never short.
        assert!(second - first >= 40_000, "{}", second - first);
    }

    #[test]
    fn the_local_time_is_a_day_and_a_minute_of_it() {
        let (day, minute) = local_day_and_minute();
        assert!(day < 7);
        assert!(minute < 24 * 60);
    }
}
