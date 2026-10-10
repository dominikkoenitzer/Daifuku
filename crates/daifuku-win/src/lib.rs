//! The Win32 half of Daifuku. Everything that asks Windows something or
//! changes something lives here, behind small safe functions, so the daemon
//! and the command line read as plain logic.
//!
//! Window handles cross this crate's boundary as `u64`: that is how they
//! travel over the pipes and how the pure logic in `daifuku-core` keys its
//! maps, and it keeps the `windows` crate out of every other signature.
//!
//! - [`access`]: high contrast, reduced motion and the notification sound.
//! - [`clock`]: a clock that setting the time does not move.
//! - [`dpi`]: per-monitor DPI awareness, which every other call depends on.
//! - [`monitor`]: the attached monitors in physical pixels.
//! - [`window`]: reading windows and placing them by their visible frame.
//! - [`process`]: the process tree and elevation.
//! - [`console`]: from a process to the terminal window it draws in.
//! - [`pipe`]: named pipes with the security each of Daifuku's two needs.
//! - [`terminal`]: finding and launching Windows Terminal.
//! - [`paths`]: where the config, the logs and the binaries live.
//! - [`setup`]: what installing needs: a locked folder and a scheduled task.
//! - [`environment`]: the machine `PATH`, and telling programs it changed.
//! - [`elevate`]: asking Windows for administrator rights, and the console.
//! - [`apps`]: the entry Settings > Apps lists Daifuku by.

#![cfg(windows)]

pub mod access;
pub mod apps;
pub mod clock;
pub mod console;
pub mod dpi;
pub mod elevate;
pub mod environment;
pub mod monitor;
pub mod paths;
pub mod pipe;
pub mod process;
pub mod setup;
pub mod terminal;
pub mod window;

mod wide;

use windows::Win32::Foundation::HWND;

/// A raw window handle as a `u64`, the form it crosses crate and process
/// boundaries in.
#[must_use]
pub fn raw(h: HWND) -> u64 {
    h.0 as usize as u64
}

/// The `HWND` for a raw handle.
#[must_use]
pub fn hwnd(raw: u64) -> HWND {
    HWND(usize::try_from(raw).unwrap_or(0) as *mut core::ffi::c_void)
}
