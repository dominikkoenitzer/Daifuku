//! Per-monitor DPI awareness.
//!
//! Without it Windows scales every coordinate Daifuku reads and writes to the
//! primary monitor's DPI. On a 4K screen at 150 % beside a 96 DPI one, a
//! process that is not aware sees the 4K screen as 2560 by 1440 and puts a
//! window on the other screen a third off target. The first thing the daemon
//! and the command line do is call [`per_monitor_v2`].

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

/// Makes the process per-monitor DPI aware (v2). Returns whether it is now,
/// which includes the case where a manifest already made it so and the call
/// itself was refused for that reason.
pub fn per_monitor_v2() -> bool {
    // SAFETY: no pointers; the call only flips process state.
    let set = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    set.is_ok() || already_aware()
}

fn already_aware() -> bool {
    use windows::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext};
    // SAFETY: both calls take and return opaque context handles.
    unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    }
}
