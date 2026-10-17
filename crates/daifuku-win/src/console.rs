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

use crate::process::{Proc, ancestors, started};

/// How much earlier than the hook the agent's process has to have started,
/// in 100 ns ticks: a second. A shell an agent starts for one hook is
/// younger; the agent itself has been running since before it ran hooks.
const AGENT_LEAD: u64 = 10_000_000;
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
    terminal_window_of(pid).map(|(w, _)| w)
}

/// [`terminal_window`], and the agent's own process in it, which ends when
/// the agent does: of the ancestors of `pid` on the terminal's console, the
/// nearest that started a second or more before `pid`, so neither the hook
/// nor a shell started just to run it; else the farthest one, the shell the
/// terminal started. For a terminal found by the window's owner, the owner.
/// The process is `None` when only the hook itself is on the console.
#[must_use]
pub fn terminal_window_of(pid: u32) -> Option<(u64, Option<u32>)> {
    let chain = ancestors(pid);
    let Some((at, w)) = chain
        .iter()
        .enumerate()
        .find_map(|(i, p)| via_console(p.pid).map(|w| (i, w)))
    else {
        return via_owning_process(&chain).map(|(w, owner)| (w, Some(owner)));
    };
    let on_console: Vec<(u32, Option<u64>)> = chain
        .iter()
        .enumerate()
        .skip(at.max(1))
        .filter(|&(i, p)| i == at || via_console(p.pid) == Some(w))
        .map(|(_, p)| (p.pid, started(p.pid)))
        .collect();
    Some((w, agent_of(&on_console, started(pid))))
}

/// Of `on_console`, processes nearest first with when each started, the
/// agent's: the nearest that started at least [`AGENT_LEAD`] before `hook`
/// did, else the farthest.
fn agent_of(on_console: &[(u32, Option<u64>)], hook: Option<u64>) -> Option<u32> {
    let old_enough = |start: Option<u64>| match (start, hook) {
        (Some(s), Some(h)) => s.saturating_add(AGENT_LEAD) <= h,
        _ => false,
    };
    on_console
        .iter()
        .find(|&&(_, start)| old_enough(start))
        .or_else(|| on_console.last())
        .map(|&(pid, _)| pid)
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
            let root = raw(GetAncestor(owner, GA_ROOT));
            if has_area(root) {
                return Some(root);
            }
        }
    }
    // The classic console host: its console window is the window. Never the
    // pseudo console window: it reports itself visible, at zero size, while a
    // new terminal is still being set up. Measured on the first demo, where
    // every agent started before its window was ready and all six named it.
    let c = raw(console);
    (window::class(c) != PSEUDO_CONSOLE && window::is_shown(c) && has_area(c)).then_some(c)
}

/// The class of the hidden window a ConPTY console has.
pub const PSEUDO_CONSOLE: &str = "PseudoConsoleWindow";

fn has_area(w: u64) -> bool {
    window::frame(w).is_some_and(|f| !f.is_empty())
}

/// The window a person sees for a window a hook named: the window itself if
/// it is a real, visible top-level window; for a pseudo console window, the
/// terminal window that owns it, which may not have existed yet when the
/// hook looked. `None` when there is nothing to draw around.
#[must_use]
pub fn visible_window(w: u64) -> Option<u64> {
    if !window::exists(w) {
        return None;
    }
    if window::class(w) == PSEUDO_CONSOLE {
        return follow(crate::hwnd(w));
    }
    // SAFETY: GetAncestor accepts any handle.
    let root = raw(unsafe { GetAncestor(crate::hwnd(w), GA_ROOT) });
    (root == w && has_area(w)).then_some(w)
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
fn via_owning_process(chain: &[Proc]) -> Option<(u64, u32)> {
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
            return Some((w, p.pid));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = 10_000_000;

    #[test]
    fn the_agent_is_the_nearest_process_that_ran_before_the_hook() {
        let hook = Some(100 * SECOND);
        // A shell started for this hook, the agent, the terminal's shell.
        let chain = [
            (10, Some(100 * SECOND - 1_000)),
            (20, Some(40 * SECOND)),
            (30, Some(5 * SECOND)),
        ];
        assert_eq!(agent_of(&chain, hook), Some(20));
        // Without the agent, the terminal's shell.
        assert_eq!(agent_of(&chain[..1], hook), Some(10));
        assert_eq!(agent_of(&[chain[0], chain[2]], hook), Some(30));
        // Just under a second is too young.
        assert_eq!(agent_of(&[(40, Some(99 * SECOND + 1))], hook), Some(40));
        assert_eq!(
            agent_of(
                &[(40, Some(99 * SECOND + 1)), (50, Some(99 * SECOND))],
                hook
            ),
            Some(50)
        );
        // Nothing on the console but the hook itself.
        assert_eq!(agent_of(&[], hook), None);
        // Without start times, the farthest.
        assert_eq!(agent_of(&[(1, None), (2, None)], None), Some(2));
    }
}
