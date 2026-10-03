//! What the daemon keeps across a restart: the open fleets, what each agent
//! is doing, and since when.
//!
//! Every install restarts the daemon, and Task Scheduler restarts one that
//! stopped on an error. Without this, the restarted daemon would know no
//! fleet, so a fleet's key would open a second one beside it, and every
//! agent would stay uncoloured until its next hook.
//!
//! Window handles mean something only on the desktop they came from, and the
//! hooks' stamps only since Windows started. A save is therefore taken back
//! only within the same start of Windows and only while it is fresh, as it is
//! after an install or a restart on error, and every window in it is checked
//! again before it is used.

use serde::{Deserialize, Serialize};

use crate::state::{AgentState, Agents};

/// How long a save stays fresh, in seconds. Longer than an install or a
/// restart on error takes; shorter than signing out and in again usually
/// does, which leaves no window of the save behind.
pub const FRESH_SECONDS: u64 = 300;

/// How far the start of Windows, worked out from the wall clock, may drift
/// between a save and its use, in seconds. A time server corrects the clock
/// by less; a restart of Windows moves it by far more.
const BOOT_DRIFT_SECONDS: u64 = 10;

/// 100 ns ticks per second.
const TICKS_PER_SECOND: u64 = 10_000_000;

/// The daemon's state as written to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    /// When Windows started, in seconds since 1970 by the wall clock.
    pub boot: u64,
    /// The interrupt time of the save, in 100 ns ticks since Windows started.
    pub at: u64,
    /// Every agent session.
    pub agents: Agents<u64>,
    /// Every open fleet.
    pub fleets: Vec<SavedFleet>,
    /// Since when each session has shown its state in its window, as an
    /// interrupt time.
    pub shown: Vec<SavedShown>,
}

/// One open fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFleet {
    /// The config's name for it.
    pub name: String,
    /// The monitor it opened on.
    pub monitor: String,
    /// The terminal in each cell, `None` for a closed one.
    pub slots: Vec<Option<u64>>,
}

/// Since when one session has shown its state in one window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedShown {
    /// The session.
    pub session: String,
    /// The window it runs in.
    pub window: u64,
    /// The state it shows.
    pub state: AgentState,
    /// Since when, as an interrupt time.
    pub since: u64,
}

/// When Windows started, in seconds since 1970, from the wall clock `now`
/// in seconds since 1970 and the interrupt time `ticks`.
#[must_use]
pub const fn boot(now: u64, ticks: u64) -> u64 {
    now.saturating_sub(ticks / TICKS_PER_SECOND)
}

impl Saved {
    /// Whether a daemon may take this save back, at the interrupt time
    /// `ticks` of a Windows that started at `boot`: the same start of
    /// Windows, and a save younger than [`FRESH_SECONDS`].
    #[must_use]
    pub const fn usable(&self, boot: u64, ticks: u64) -> bool {
        let same_start = self.boot.abs_diff(boot) <= BOOT_DRIFT_SECONDS && self.at <= ticks;
        same_start && ticks - self.at <= FRESH_SECONDS * TICKS_PER_SECOND
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HookEvent;

    const HOUR: u64 = 3600 * TICKS_PER_SECOND;

    fn saved(boot: u64, at: u64) -> Saved {
        Saved {
            boot,
            at,
            agents: Agents::new(),
            fleets: Vec::new(),
            shown: Vec::new(),
        }
    }

    #[test]
    fn a_fresh_save_from_this_start_of_windows_is_taken_back() {
        let s = saved(1_000, HOUR);
        assert!(s.usable(1_000, HOUR + 5 * TICKS_PER_SECOND));
        // A time server set the clock a few seconds.
        assert!(s.usable(1_003, HOUR + 5 * TICKS_PER_SECOND));
    }

    #[test]
    fn a_save_from_another_start_of_windows_is_not() {
        let s = saved(1_000, HOUR);
        // Windows started again later; its interrupt time began anew.
        assert!(!s.usable(1_000 + 3_600, 30 * TICKS_PER_SECOND));
        // Started again, and already up longer than at the save.
        assert!(!s.usable(1_000 + 600, HOUR + TICKS_PER_SECOND));
        // An interrupt time behind the save's is another start, whatever
        // the wall clock says.
        assert!(!s.usable(1_000, HOUR - 1));
    }

    #[test]
    fn a_stale_save_is_not() {
        let s = saved(1_000, HOUR);
        assert!(s.usable(1_000, HOUR + FRESH_SECONDS * TICKS_PER_SECOND));
        assert!(!s.usable(1_000, HOUR + FRESH_SECONDS * TICKS_PER_SECOND + 1));
    }

    #[test]
    fn the_start_of_windows_is_the_wall_clock_less_the_interrupt_time() {
        assert_eq!(boot(10_000, 600 * TICKS_PER_SECOND), 9_400);
        assert_eq!(boot(10, 600 * TICKS_PER_SECOND), 0);
    }

    #[test]
    fn agents_round_trip_through_json() {
        let mut agents = Agents::new();
        agents.apply(
            7,
            &HookEvent {
                session_id: "s".into(),
                hook_event_name: "PermissionRequest".into(),
                daifuku_at: Some(42),
                ..HookEvent::default()
            },
        );
        let s = Saved {
            agents,
            fleets: vec![SavedFleet {
                name: "agents".into(),
                monitor: "portrait".into(),
                slots: vec![Some(7), None],
            }],
            shown: vec![SavedShown {
                session: "s".into(),
                window: 7,
                state: AgentState::Waiting,
                since: 40,
            }],
            ..saved(1_000, HOUR)
        };
        let back: Saved = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.agents.window_state(7), Some(AgentState::Waiting));
        assert_eq!(back.agents.needs_you(), vec![7]);
        assert_eq!(back.fleets, s.fleets);
        assert_eq!(back.shown, s.shown);
        // The stamp came along: an event sent before it is still late.
        let mut agents = back.agents;
        let late = HookEvent {
            session_id: "s".into(),
            hook_event_name: "PostToolUse".into(),
            daifuku_at: Some(41),
            ..HookEvent::default()
        };
        assert!(!agents.apply(7, &late));
    }
}
