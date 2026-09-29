//! From a process to the terminal window it draws in.
//!
//! A hook runs as a child of the agent, and the agent runs in a console. That
//! console's window is not the terminal: under Windows Terminal it is a hidden
//! `PseudoConsoleWindow`, and every Windows Terminal window on the desktop
//! belongs to one and the same `WindowsTerminal.exe`, so the process id alone
//! cannot say which window. What does say it is the pseudo console window's
//! **owner**: Windows Terminal makes the window hosting the tab its owner.
//!
//! The hook itself usually has no console of its own (the agent starts it
//! without one), so [`terminal_window`] walks up the process tree and borrows
//! each ancestor's console in turn with `AttachConsole` until one leads to a
//! window. Measured on Windows 11 with Windows Terminal: the agent's own
//! process (`claude.exe`) is the first ancestor whose console answers.

use windows::Win32::System::Console::{AttachConsole, FreeConsole, GetConsoleWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GW_OWNER, GetAncestor, GetWindow, IsWindowVisible,
};

use crate::process::{Proc, ancestors};
use crate::{raw, window};

/// The top-level window of the terminal `pid` runs in, looking at `pid`
/// itself and then at each ancestor.
///
/// Returns `None` when no ancestor has a console that leads to a visible
/// window, which is the case for an agent started by a service or from a
/// terminal that hides itself.
///
/// This detaches the calling process from its own console. Call it from a
/// process that is done writing to its console, as the hook is.
#[must_use]
pub fn terminal_window(pid: u32) -> Option<u64> {
    let chain = ancestors(pid);
    for p in &chain {
        if let Some(w) = via_console(p.pid) {
            return Some(w);
        }
    }
    via_owning_process(&chain)
}

/// The terminal window of the console this process is attached to, without
/// letting go of it.
///
/// For a program that keeps writing to its console, like the demo agent.
/// [`terminal_window`] detaches the caller from its own console to borrow
/// others', which is right for a hook that is about to exit and wrong for
/// anything that prints afterwards: its output would go nowhere and it would
/// outlive its window. Measured: that is exactly what the first demo did.
#[must_use]
pub fn own_terminal_window() -> Option<u64> {
    // SAFETY: only reads the handle of the console already attached.
    let console = unsafe { GetConsoleWindow() };
    if console.is_invalid() {
        return None;
    }
    follow(console)
}

/// From a console window to the top-level window a person sees.
fn follow(console: windows::Win32::Foundation::HWND) -> Option<u64> {
    // SAFETY: the handles are only read.
    unsafe {
        // Windows Terminal and other ConPTY hosts: the owner is the window.
        if let Ok(owner) = GetWindow(console, GW_OWNER)
            && !owner.is_invalid()
            && IsWindowVisible(owner).as_bool()
        {
            return Some(raw(GetAncestor(owner, GA_ROOT)));
        }
        // The classic console host: its console window is the window.
        if IsWindowVisible(console).as_bool() {
            return Some(raw(console));
        }
    }
    None
}

/// Borrows `pid`'s console and follows it to a window.
fn via_console(pid: u32) -> Option<u64> {
    // SAFETY: FreeConsole and AttachConsole only change which console this
    // process is attached to; the window handles are only read.
    unsafe {
        let _ = FreeConsole();
        AttachConsole(pid).ok()?;
        let console = GetConsoleWindow();
        let _ = FreeConsole();
        if console.is_invalid() {
            return None;
        }
        follow(console)
    }
}

/// The fallback for terminals whose pseudo console has no owner, such as an
/// editor's built-in terminal: the nearest ancestor that owns a visible
/// top-level window, and of its windows the largest.
fn via_owning_process(chain: &[Proc]) -> Option<u64> {
    let windows = window::top_level();
    for p in chain {
        let best = windows
            .iter()
            .copied()
            .filter(|&w| window::pid(w) == p.pid && window::is_shown(w))
            .filter_map(|w| {
                window::frame(w).map(|f| (w, i64::from(f.width()) * i64::from(f.height())))
            })
            .max_by_key(|&(_, area)| area);
        if let Some((w, _)) = best {
            return Some(w);
        }
    }
    None
}
