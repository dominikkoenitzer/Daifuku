//! Reading top-level windows and placing them.
//!
//! Placement works on the **visible frame**, `DWMWA_EXTENDED_FRAME_BOUNDS`,
//! not on the rectangle `SetWindowPos` takes. On Windows 10 and 11 a window's
//! rectangle includes an invisible resize border, several pixels wide and
//! different per DPI, so placing by the window rectangle leaves gaps that are
//! uneven by exactly that much. [`place`] measures the difference and pays it,
//! and measures again, because a window that crosses onto a monitor with a
//! different DPI rescales itself after the first move.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use daifuku_core::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed, PostMessageW,
    SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos, ShowWindow,
    WM_CLOSE,
};

use crate::monitor::rect;
use crate::wide::from_wide;
use crate::{hwnd, raw};

/// Every top-level window, visible or not, in z-order from the top.
#[must_use]
pub fn top_level() -> Vec<u64> {
    let mut list: Vec<u64> = Vec::with_capacity(256);
    // SAFETY: the callback only runs during this call; lparam points at list.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&raw mut list as isize));
    }
    list
}

unsafe extern "system" fn collect(h: HWND, lparam: LPARAM) -> windows::core::BOOL {
    // SAFETY: lparam is the Vec handed to EnumWindows above.
    let list = unsafe { &mut *(lparam.0 as *mut Vec<u64>) };
    list.push(raw(h));
    true.into()
}

/// Whether the handle still names a window. Handles are reused, so this says
/// nothing about whether it is the same window as before.
#[must_use]
pub fn exists(w: u64) -> bool {
    // SAFETY: IsWindow accepts any value.
    w != 0 && unsafe { IsWindow(Some(hwnd(w))) }.as_bool()
}

/// Visible in the `WS_VISIBLE` sense and not cloaked (another virtual desktop,
/// a window manager hiding it, a suspended app).
#[must_use]
pub fn is_shown(w: u64) -> bool {
    // SAFETY: plain query on a handle.
    unsafe { IsWindowVisible(hwnd(w)) }.as_bool() && !is_cloaked(w)
}

/// Cloaked by DWM, for any reason.
#[must_use]
pub fn is_cloaked(w: u64) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the buffer is a u32, which is what DWMWA_CLOAKED writes.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd(w),
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast::<c_void>(),
            u32::try_from(size_of::<u32>()).unwrap_or(4),
        )
    };
    ok.is_ok() && cloaked != 0
}

/// Minimised.
#[must_use]
pub fn is_minimised(w: u64) -> bool {
    // SAFETY: plain query on a handle.
    unsafe { IsIconic(hwnd(w)) }.as_bool()
}

/// The window class, `CASCADIA_HOSTING_WINDOW_CLASS` for Windows Terminal.
#[must_use]
pub fn class(w: u64) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: buf is a valid, sized buffer.
    let n = unsafe { GetClassNameW(hwnd(w), &mut buf) };
    from_wide(&buf[..usize::try_from(n).unwrap_or(0).min(buf.len())])
}

/// The title bar text.
#[must_use]
pub fn title(w: u64) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: buf is a valid, sized buffer.
    let n = unsafe { GetWindowTextW(hwnd(w), &mut buf) };
    from_wide(&buf[..usize::try_from(n).unwrap_or(0).min(buf.len())])
}

/// The process that owns the window.
#[must_use]
pub fn pid(w: u64) -> u32 {
    let mut pid = 0u32;
    // SAFETY: pid is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd(w), Some(&raw mut pid)) };
    pid
}

/// The window rectangle, invisible resize borders included.
#[must_use]
pub fn outer(w: u64) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: r is a valid out pointer.
    unsafe { GetWindowRect(hwnd(w), &raw mut r) }
        .ok()
        .map(|()| rect(r))
}

/// The visible frame: what a person sees as the window's edge.
#[must_use]
pub fn frame(w: u64) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: the buffer is a RECT, which DWMWA_EXTENDED_FRAME_BOUNDS writes.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd(w),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut r).cast::<c_void>(),
            u32::try_from(size_of::<RECT>()).unwrap_or(16),
        )
    };
    ok.ok().map(|()| rect(r))
}

/// The window rectangle that gives a visible frame of `target`, for a window
/// whose rectangle is `outer` while its frame is `frame`.
#[must_use]
pub const fn outer_for(target: Rect, outer: Rect, frame: Rect) -> Rect {
    Rect::new(
        target.left - (frame.left - outer.left),
        target.top - (frame.top - outer.top),
        target.right + (outer.right - frame.right),
        target.bottom + (outer.bottom - frame.bottom),
    )
}

/// Moves and sizes the window so its visible frame is exactly `target`, and
/// returns the frame it ended up with.
///
/// A minimised or maximised window is restored first, because Windows keeps
/// both in a rectangle of their own and would ignore the move. The loop runs
/// until the frame matches or three passes are spent: the second pass is for
/// the DPI change a window goes through when it lands on another monitor,
/// the third is slack for a slow app.
pub fn place(w: u64, target: Rect) -> Option<Rect> {
    let h = hwnd(w);
    // SAFETY: plain calls on a handle.
    unsafe {
        if IsIconic(h).as_bool() || IsZoomed(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
    }
    for _ in 0..3 {
        let (Some(o), Some(f)) = (outer(w), frame(w)) else {
            return None;
        };
        if f == target {
            return Some(f);
        }
        // Before the first move the frame is where the window was, which is
        // fine: the border widths it gives are what they are on that
        // monitor, and the next pass corrects for the one it lands on.
        let want = outer_for(target, o, f);
        // SAFETY: plain call on a handle.
        let moved = unsafe {
            SetWindowPos(
                h,
                None,
                want.left,
                want.top,
                want.width(),
                want.height(),
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        if moved.is_err() {
            return frame(w);
        }
        wait_for_frame(w, target, Duration::from_millis(60));
    }
    frame(w)
}

/// Polls the frame for up to `limit`, returning as soon as it is `target`.
/// A window that crossed monitors rescales asynchronously, a few
/// milliseconds after the move returned.
fn wait_for_frame(w: u64, target: Rect, limit: Duration) {
    let start = Instant::now();
    while start.elapsed() < limit {
        if frame(w) == Some(target) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The foreground window.
#[must_use]
pub fn foreground() -> u64 {
    // SAFETY: no arguments.
    raw(unsafe { GetForegroundWindow() })
}

/// Brings a window to the front and gives it the keyboard.
///
/// Windows refuses `SetForegroundWindow` to a process that did not receive the
/// last input. A hotkey handler did, so from there it works; for the other
/// paths a single Alt tap counts as input and unlocks it, which is the
/// technique the shell's own task switcher relies on. The tap is a key up and
/// down of Alt alone, which no program treats as a shortcut.
pub fn focus(w: u64) -> bool {
    let h = hwnd(w);
    // SAFETY: plain calls on a handle.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        if SetForegroundWindow(h).as_bool() {
            return true;
        }
        tap_alt();
        SetForegroundWindow(h).as_bool()
    }
}

fn tap_alt() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_MENU,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
    // SAFETY: inputs is a valid array of INPUT with the right size.
    unsafe {
        SendInput(&inputs, i32::try_from(size_of::<INPUT>()).unwrap_or(0));
    }
}

/// Asks the window to close, the way its close button would.
pub fn close(w: u64) -> bool {
    // SAFETY: posting a message with no pointers in it.
    unsafe { PostMessageW(Some(hwnd(w)), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_for_pays_the_invisible_border_on_each_side() {
        // A Windows 11 window at 100 %: seven invisible pixels left, right and
        // below, none on top.
        let outer = Rect::new(93, 100, 507, 407);
        let frame = Rect::new(100, 100, 500, 400);
        let target = Rect::new(3864, 144, 4370, 738);
        let want = outer_for(target, outer, frame);
        assert_eq!(want, Rect::new(3857, 144, 4377, 745));
    }

    #[test]
    fn with_no_border_outer_for_is_the_target() {
        let r = Rect::new(10, 10, 20, 20);
        assert_eq!(
            outer_for(Rect::new(0, 0, 5, 5), r, r),
            Rect::new(0, 0, 5, 5)
        );
    }

    #[test]
    fn a_dead_handle_is_harmless() {
        assert!(!exists(0));
        assert!(frame(0).is_none());
        assert!(place(0, Rect::new(0, 0, 10, 10)).is_none());
    }
}
