//! A clock for putting events in order that setting the time does not move.

use windows::Win32::System::WindowsProgramming::QueryInterruptTimePrecise;

/// The interrupt time: 100 ns ticks since Windows started, the same in every
/// process. Unlike the wall clock it never steps back, whether the time is
/// set by hand, by a time zone or by a sync with a time server.
#[must_use]
pub fn ticks() -> u64 {
    // SAFETY: no arguments; only reads the system's interrupt time.
    unsafe { QueryInterruptTimePrecise() }
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
}
