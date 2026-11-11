//! A clock for putting events in order that setting the time does not move,
//! which start of Windows it counts from, and the local time of day.

use std::sync::OnceLock;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::WindowsProgramming::QueryInterruptTimePrecise;
use windows::core::w;

/// The interrupt time: 100 ns ticks since Windows started, the same in every
/// process. Unlike the wall clock it never steps back, whether the time is
/// set by hand, by a time zone or by a sync with a time server.
#[must_use]
pub fn ticks() -> u64 {
    // SAFETY: no arguments; only reads the system's interrupt time.
    unsafe { QueryInterruptTimePrecise() }
}

/// Which start of Windows this is: the number Windows counts up each time it
/// starts, 0 where it does not read. It names a start without saying when
/// that was, so a file that keeps it holds no clock time.
#[must_use]
pub fn boot_id() -> u64 {
    static ID: OnceLock<u64> = OnceLock::new();
    *ID.get_or_init(|| {
        let mut id = 0u32;
        let mut size: u32 = 4;
        // SAFETY: both names are NUL terminated literals; the buffer is a
        // u32 and `size` says so, which a REG_DWORD fills exactly.
        let read = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters"),
                w!("BootId"),
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut id).cast()),
                Some(&raw mut size),
            )
        };
        if read == ERROR_SUCCESS { u64::from(id) } else { 0 }
    })
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
    fn this_start_of_windows_has_an_id() {
        let id = boot_id();
        assert_ne!(id, 0);
        assert_eq!(boot_id(), id);
    }

    #[test]
    fn the_local_time_is_a_day_and_a_minute_of_it() {
        let (day, minute) = local_day_and_minute();
        assert!(day < 7);
        assert!(minute < 24 * 60);
    }
}
