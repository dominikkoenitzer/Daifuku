//! Reading top-level windows and placing them.
//!
//! Placement works on the **visible frame**, `DWMWA_EXTENDED_FRAME_BOUNDS`,
//! not on the rectangle `SetWindowPos` takes. On Windows 10 and 11 a window's
//! rectangle includes an invisible resize border, several pixels wide and
//! different per DPI, so placing by the window rectangle leaves gaps that are
//! uneven by exactly that much. [`place`] measures the difference and pays it,
//! and measures again, because a window that crosses onto a monitor with a
//! different DPI rescales itself after the first move.
//!
//! Moving, showing or raising a window that another thread owns waits for
//! that thread to handle it, and so does a switch made with the foreground
//! thread's input queue joined. A window whose app has stopped answering
//! would hold the daemon's message loop for as long as it hangs, so
//! [`place`] and [`focus`] first check that the thread is taking messages:
//! [`place`] moves a window whose thread is not by a queued request instead,
//! and [`focus`] leaves it alone.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use daifuku_core::Rect;
use windows::Win32::Foundation::{
    ERROR_TIMEOUT, GetLastError, HWND, LPARAM, RECT, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible,
    IsZoomed, PostMessageW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SW_RESTORE, SWP_ASYNCWINDOWPOS,
    SWP_NOACTIVATE, SWP_NOZORDER, SendMessageTimeoutW, SetForegroundWindow, SetWindowPos,
    ShowWindow, ShowWindowAsync, WM_CLOSE, WM_NULL,
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
///
/// A window whose thread is not answering is not waited for: the move is
/// only queued, to happen when the thread answers again, and the result is
/// `None`.
pub fn place(w: u64, target: Rect) -> Option<Rect> {
    let h = hwnd(w);
    if !answers(h) {
        return place_later(w, target);
    }
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

/// Queues the move for a window whose thread is not answering, so it happens
/// when the thread answers again, and returns `None` because the frame it
/// will have is not known yet.
///
/// There is one queued request and no read-back, so the border widths are
/// taken from where the window is now. A minimised or maximised window is
/// only queued to restore: its border widths are not the ones it will have
/// restored, and the next placement puts it in its cell.
fn place_later(w: u64, target: Rect) -> Option<Rect> {
    let h = hwnd(w);
    // SAFETY: plain calls on a handle; the restore and the move are only
    // posted to the thread that owns it.
    unsafe {
        if IsIconic(h).as_bool() || IsZoomed(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
            return None;
        }
        let want = outer_for(target, outer(w)?, frame(w)?);
        let _ = SetWindowPos(
            h,
            None,
            want.left,
            want.top,
            want.width(),
            want.height(),
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
    None
}

/// How long [`answers`] gives a window's thread to handle a message, in
/// milliseconds.
const ANSWER_TIMEOUT_MS: u32 = 200;

/// Whether the thread that owns the window is taking messages: Windows has
/// not marked it hung, and it handles a `WM_NULL`, which does nothing,
/// within [`ANSWER_TIMEOUT_MS`].
///
/// Windows marks a thread hung only after five seconds without a message,
/// so the `WM_NULL` is what catches an app that stopped answering just now.
fn answers(h: HWND) -> bool {
    // SAFETY: plain calls on a handle; the message carries no pointers.
    unsafe {
        if IsHungAppWindow(h).as_bool() {
            return false;
        }
        let sent = SendMessageTimeoutW(
            h,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            ANSWER_TIMEOUT_MS,
            None,
        );
        answered(sent.0 != 0, GetLastError())
    }
}

/// Whether a `SendMessageTimeoutW` that returned `sent`, with `error` as the
/// last error, means the thread answered. Only a timeout means it did not: a
/// message refused for another reason, such as a window of a higher
/// integrity level, says nothing about the thread, and the call it was a
/// check for goes ahead as before.
const fn answered(sent: bool, error: WIN32_ERROR) -> bool {
    sent || error.0 != ERROR_TIMEOUT.0
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

/// Brings a window to the front and gives it the keyboard, and says whether
/// it is still in front a moment later.
///
/// Windows guards the foreground: `SetForegroundWindow` from a process that
/// did not receive the last input can report success and still only flash
/// the taskbar button, and a switch that did happen can be taken back by the
/// window that lost it. Both were measured on the first runs. So each attempt
/// does what reliably works, then waits for the switch to settle and checks:
///
/// 1. Joins the input queue of the thread that owns the foreground window
///    for the duration of the call (`AttachThreadInput`), which makes the
///    switch look like it came from the foreground's own input.
/// 2. From the second attempt on, first taps Alt once (a key up and down of
///    Alt alone, which no program treats as a shortcut) so this process has
///    the last input.
///
/// A window whose thread is not answering is left alone and the result is
/// `false`: restoring and raising it would wait for that thread.
pub fn focus(w: u64) -> bool {
    let h = hwnd(w);
    if !answers(h) {
        return false;
    }
    // SAFETY: plain call on a handle.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
    }
    for attempt in 0..3 {
        if foreground() != w {
            if attempt > 0 {
                tap_alt();
            }
            switch_to(h);
        }
        if settled(w) {
            return true;
        }
    }
    false
}

/// One switch attempt, with the foreground thread's input queue joined.
///
/// The queue is not joined when the foreground's thread is not answering:
/// joined, the activation is handled inside that thread's queue and waits
/// for it. The attempt then goes ahead without it, and the Alt tap of the
/// later attempts still gives this process the last input.
fn switch_to(h: HWND) {
    // SAFETY: plain calls on handles and thread ids; the attachment is always
    // undone before returning.
    unsafe {
        let front = GetForegroundWindow();
        let front_thread = GetWindowThreadProcessId(front, None);
        let me = GetCurrentThreadId();
        let attached = front_thread != 0
            && front_thread != me
            && answers(front)
            && AttachThreadInput(me, front_thread, true).as_bool();
        let _ = BringWindowToTop(h);
        let _ = SetForegroundWindow(h);
        if attached {
            let _ = AttachThreadInput(me, front_thread, false);
        }
    }
}

/// Whether `w` is in front now and still is 80 ms later, which is longer
/// than a window that lost the foreground takes to grab it back.
fn settled(w: u64) -> bool {
    for _ in 0..4 {
        if foreground() != w {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    foreground() == w
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
    fn only_a_timeout_means_a_window_is_not_answering() {
        assert!(answered(true, WIN32_ERROR(0)));
        assert!(!answered(false, ERROR_TIMEOUT));
        assert!(answered(
            false,
            windows::Win32::Foundation::ERROR_ACCESS_DENIED
        ));
    }

    #[test]
    fn a_dead_handle_is_harmless() {
        assert!(!exists(0));
        assert!(frame(0).is_none());
        assert!(place(0, Rect::new(0, 0, 10, 10)).is_none());
    }
}
