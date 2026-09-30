//! Window events through `SetWinEventHook`, out of context.
//!
//! Out of context means no DLL is injected anywhere: Windows queues each event
//! and delivers it to this thread's message loop. The callback only records
//! the window and the kind of event; the main loop drains the queue after
//! every message and does the work there.
//!
//! One trap, found on the first run: Windows calls the callback from inside
//! `GetMessageW`, and `GetMessageW` does not return for it. A border then only
//! caught up with its window on the next timer tick, two seconds later. So the
//! first event into an empty queue posts [`WM_EVENTS`] to the thread, which
//! makes `GetMessageW` return and the loop drain the queue at once.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, OBJID_WINDOW, PostThreadMessageW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_APP,
};

/// Posted to the main thread when the event queue goes from empty to not.
pub const WM_EVENTS: u32 = WM_APP + 2;

/// What happened to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// It is gone. Its handle may be reused from now on.
    Destroyed(u64),
    /// It moved or changed size.
    Moved(u64),
    /// It was shown, hidden, cloaked, uncloaked, minimised or restored.
    Visibility(u64),
    /// It came to the foreground, and with it to the top of the z-order.
    Foreground(u64),
}

thread_local! {
    static QUEUE: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

/// Takes every event recorded since the last call.
pub fn drain() -> Vec<Event> {
    QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// The events a border follows its window by: shown and hidden, moved,
/// cloaked and uncloaked, minimised and restored, and the foreground.
const FOLLOW: [(u32, u32); 5] = [
    (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
    (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
    (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
    (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
];

/// The installed hooks; dropping it removes them.
///
/// The destroy hook is always in: destroys are rare, and they are how agents
/// and fleets let go of a closed window. The ones a border follows its window
/// by are only in while [`Hooks::follow`] asks for them, because a move is
/// reported for every caret and every mouse movement on the whole desktop,
/// and each one wakes this thread whether any window has a border or not.
pub struct Hooks {
    always: Vec<HWINEVENTHOOK>,
    following: Option<Vec<HWINEVENTHOOK>>,
}

impl Hooks {
    /// Installs the destroy hook for the calling thread, which must run a
    /// message loop for any event to arrive.
    pub fn install() -> Self {
        Self {
            always: hook(&[(EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY)]),
            following: None,
        }
    }

    /// Installs the hooks borders follow their windows by when `on`, and
    /// removes them when not. Nothing happens when they already are as asked,
    /// so it is cheap to call on every pass. Must be called on the thread
    /// that installed the hooks.
    pub fn follow(&mut self, on: bool) {
        match (on, self.following.take()) {
            (true, None) => {
                tracing::debug!("following windows");
                self.following = Some(hook(&FOLLOW));
            }
            (false, Some(hooks)) => {
                tracing::debug!("no longer following windows");
                unhook(hooks);
            }
            (_, kept) => self.following = kept,
        }
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        unhook(std::mem::take(&mut self.always));
        if let Some(hooks) = self.following.take() {
            unhook(hooks);
        }
    }
}

/// Installs one out-of-context hook per range of events.
fn hook(ranges: &[(u32, u32)]) -> Vec<HWINEVENTHOOK> {
    let mut hooks = Vec::new();
    for &(min, max) in ranges {
        // SAFETY: out-of-context hook with a static callback, removed by
        // `unhook` on this same thread.
        let h = unsafe {
            SetWinEventHook(
                min,
                max,
                None,
                Some(callback),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if h.is_invalid() {
            tracing::warn!(
                min,
                max,
                "SetWinEventHook failed; borders will not follow those events"
            );
        } else {
            hooks.push(h);
        }
    }
    hooks
}

/// Removes hooks this thread installed.
fn unhook(hooks: Vec<HWINEVENTHOOK>) {
    for h in hooks {
        // SAFETY: a hook this thread installed.
        let _ = unsafe { UnhookWinEvent(h) };
    }
}

unsafe extern "system" fn callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    // Only the window itself: not its caret, cursor, scroll bars or children.
    if object != OBJID_WINDOW.0 || child != 0 || hwnd.is_invalid() {
        return;
    }
    let w = daifuku_win::raw(hwnd);
    let e = match event {
        EVENT_OBJECT_DESTROY => Event::Destroyed(w),
        EVENT_OBJECT_LOCATIONCHANGE => Event::Moved(w),
        EVENT_SYSTEM_FOREGROUND => Event::Foreground(w),
        _ => Event::Visibility(w),
    };
    let first = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let first = q.is_empty();
        // A drag fires a move per frame; one pending per window is enough.
        if !q.contains(&e) {
            q.push(e);
        }
        first
    });
    if first {
        // SAFETY: posting a message with no pointers to this very thread.
        unsafe {
            let _ = PostThreadMessageW(GetCurrentThreadId(), WM_EVENTS, WPARAM(0), LPARAM(0));
        }
    }
}
