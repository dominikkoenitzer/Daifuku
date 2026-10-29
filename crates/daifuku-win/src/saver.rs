//! The screen saver, started when Windows would have started it.
//!
//! Windows starts the screen saver's countdown over whenever a new window
//! comes up active, and a new Windows Terminal window always does, whatever
//! show command its starter passes. A terminal Daifuku opens while nobody is
//! at the desk would hold the saver off for another full timeout, and peers
//! opened a few minutes apart all night would keep the screen lit. The
//! window is no input, so the last input stays where it was: [`step`] tells
//! the daemon to ask for the saver once that input is as old as the
//! timeout, unless something holds the display on, as Windows itself does.

use std::time::Duration;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::SC_SCREENSAVE;
use windows::Win32::System::Power::{
    CallNtPowerInformation, ES_DISPLAY_REQUIRED, EXECUTION_STATE, SystemExecutionState,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetDesktopWindow, SPI_GETSCREENSAVEACTIVE, SPI_GETSCREENSAVERRUNNING,
    SPI_GETSCREENSAVETIMEOUT, SYSTEM_PARAMETERS_INFO_ACTION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SystemParametersInfoW, WM_SYSCOMMAND,
};

/// What Windows says about the saver and the input at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    /// When the last input came, in [`GetTickCount`] milliseconds.
    pub last_input: u32,
    /// How long ago that was.
    pub idle: Duration,
    /// The saver's timeout, `None` when the saver is off.
    pub timeout: Option<Duration>,
    /// Whether the saver is up.
    pub running: bool,
    /// Whether a program holds the display on, as a video player does.
    pub display_held: bool,
}

impl Now {
    #[must_use]
    pub fn read() -> Self {
        let last_input = last_input();
        // SAFETY: no arguments; only reads the tick count.
        let ticks = unsafe { GetTickCount() };
        Self {
            last_input,
            idle: Duration::from_millis(u64::from(ticks.wrapping_sub(last_input))),
            timeout: timeout(),
            running: flag(SPI_GETSCREENSAVERRUNNING),
            display_held: display_held(),
        }
    }
}

/// What to do about a saver a window of Daifuku's may have put off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// Leave it to Windows: someone used the machine since, the saver is up
    /// or off, or nobody is signed in to the desktop.
    Forget,
    Start,
}

/// The step for a window opened when the last input came at `since`.
#[must_use]
pub fn step(since: u32, now: &Now, away: bool) -> Step {
    let Some(timeout) = now.timeout else {
        return Step::Forget;
    };
    if now.last_input != since || now.running || away {
        Step::Forget
    } else if now.idle < timeout || now.display_held {
        Step::Wait
    } else {
        Step::Start
    }
}

/// Asks Windows to start the saver now, as the system menu command does.
/// Nothing happens when no saver is chosen.
pub fn start() {
    // SAFETY: the desktop window always exists; the default handling of the
    // command starts the saver and touches nothing else.
    unsafe {
        DefWindowProcW(
            GetDesktopWindow(),
            WM_SYSCOMMAND,
            WPARAM(SC_SCREENSAVE as usize),
            LPARAM(0),
        );
    }
}

/// Whether nobody has touched the machine for as long as the saver waits,
/// so the saver is due or already up. `false` when the saver is off.
#[must_use]
pub fn due() -> bool {
    let now = Now::read();
    now.timeout.is_some_and(|t| now.idle >= t)
}

/// The last input, in [`GetTickCount`] milliseconds.
#[must_use]
pub fn last_input() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: u32::try_from(size_of::<LASTINPUTINFO>()).unwrap_or(0),
        dwTime: 0,
    };
    // SAFETY: info is a LASTINPUTINFO with its size set, as the call requires.
    let _ = unsafe { GetLastInputInfo(&raw mut info) };
    info.dwTime
}

fn timeout() -> Option<Duration> {
    if !flag(SPI_GETSCREENSAVEACTIVE) {
        return None;
    }
    let mut seconds = 0u32;
    // SAFETY: seconds is the u32 the call writes.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETSCREENSAVETIMEOUT,
            0,
            Some((&raw mut seconds).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    (ok.is_ok() && seconds > 0).then(|| Duration::from_secs(u64::from(seconds)))
}

fn flag(action: SYSTEM_PARAMETERS_INFO_ACTION) -> bool {
    let mut on = windows::core::BOOL(0);
    // SAFETY: on is the BOOL the call writes.
    let ok = unsafe {
        SystemParametersInfoW(
            action,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && on.as_bool()
}

/// Whether any program holds the display on, by an execution state or a
/// power request; both show in the system's execution state.
fn display_held() -> bool {
    let mut state = EXECUTION_STATE(0);
    // SAFETY: state is the EXECUTION_STATE the call writes, with its size.
    let status = unsafe {
        CallNtPowerInformation(
            SystemExecutionState,
            None,
            0,
            Some((&raw mut state).cast()),
            u32::try_from(size_of::<EXECUTION_STATE>()).unwrap_or(0),
        )
    };
    status.is_ok() && state.contains(ES_DISPLAY_REQUIRED)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEN_MINUTES: Duration = Duration::from_secs(600);

    fn now(idle_secs: u64) -> Now {
        Now {
            last_input: 1000,
            idle: Duration::from_secs(idle_secs),
            timeout: Some(TEN_MINUTES),
            running: false,
            display_held: false,
        }
    }

    #[test]
    fn starts_once_the_input_is_as_old_as_the_timeout() {
        assert_eq!(step(1000, &now(599), false), Step::Wait);
        assert_eq!(step(1000, &now(600), false), Step::Start);
        assert_eq!(step(1000, &now(4000), false), Step::Start);
    }

    #[test]
    fn new_input_leaves_it_to_windows() {
        assert_eq!(step(900, &now(700), false), Step::Forget);
    }

    #[test]
    fn a_held_display_waits() {
        let held = Now {
            display_held: true,
            ..now(700)
        };
        assert_eq!(step(1000, &held, false), Step::Wait);
    }

    #[test]
    fn a_saver_that_is_up_off_or_locked_away_is_left_alone() {
        let running = Now {
            running: true,
            ..now(700)
        };
        let off = Now {
            timeout: None,
            ..now(700)
        };
        assert_eq!(step(1000, &running, false), Step::Forget);
        assert_eq!(step(1000, &off, false), Step::Forget);
        assert_eq!(step(1000, &now(700), true), Step::Forget);
    }

    #[test]
    fn reads_without_failing() {
        let now = Now::read();
        assert!(now.timeout.is_none_or(|t| t > Duration::ZERO));
    }
}
