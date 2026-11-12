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

/// 100 ns ticks per second.
pub const TICKS_PER_SECOND: u64 = 10_000_000;

/// A moment as the daemon keeps it on disk without a clock time: the start
/// of Windows by its boot id, and the interrupt time in it. One from another
/// start of Windows, or from a version that kept none, has no age.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// The boot id of its start of Windows, 0 for none known.
    pub boot_id: u64,
    /// The interrupt time, in 100 ns ticks since Windows started.
    pub ticks: u64,
}

impl Stamp {
    /// Its age at `now`, in whole seconds, when both are of the same known
    /// start of Windows.
    #[must_use]
    pub const fn seconds_before(self, now: Self) -> Option<u64> {
        if self.boot_id == 0 || self.boot_id != now.boot_id || self.ticks > now.ticks {
            return None;
        }
        Some((now.ticks - self.ticks) / TICKS_PER_SECOND)
    }
}

/// The daemon's state as written to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    /// The boot id of the start of Windows it was saved in, 0 in a save
    /// from a version that kept the start as a clock time.
    #[serde(default)]
    pub boot_id: u64,
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

impl Saved {
    /// Whether a daemon may take this save back, at the interrupt time
    /// `ticks` of the start of Windows `boot_id` names: the same start, and
    /// a save younger than [`FRESH_SECONDS`].
    #[must_use]
    pub const fn usable(&self, boot_id: u64, ticks: u64) -> bool {
        let same_start = self.boot_id != 0 && self.boot_id == boot_id && self.at <= ticks;
        same_start && ticks - self.at <= FRESH_SECONDS * TICKS_PER_SECOND
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HookEvent;

    const HOUR: u64 = 3600 * TICKS_PER_SECOND;

    fn saved(boot_id: u64, at: u64) -> Saved {
        Saved {
            boot_id,
            at,
            agents: Agents::new(),
            fleets: Vec::new(),
            shown: Vec::new(),
        }
    }

    #[test]
    fn a_fresh_save_from_this_start_of_windows_is_taken_back() {
        let s = saved(290, HOUR);
        assert!(s.usable(290, HOUR));
        assert!(s.usable(290, HOUR + 5 * TICKS_PER_SECOND));
    }

    #[test]
    fn a_save_from_another_start_of_windows_is_not() {
        let s = saved(290, HOUR);
        // Windows started again; its interrupt time began anew.
        assert!(!s.usable(291, 30 * TICKS_PER_SECOND));
        // Started again, and already up longer than at the save.
        assert!(!s.usable(291, HOUR + TICKS_PER_SECOND));
        // An interrupt time behind the save's is another start, whatever
        // the boot id says.
        assert!(!s.usable(290, HOUR - 1));
    }

    #[test]
    fn a_save_with_no_start_of_windows_is_not() {
        // From a version that kept the start as a clock time.
        let text = serde_json::to_string(&saved(290, 1))
            .unwrap()
            .replace(r#""boot_id":290"#, r#""boot":1762851000"#);
        assert!(text.contains(r#""boot":"#), "{text}");
        let old: Saved = serde_json::from_str(&text).unwrap();
        assert_eq!(old.boot_id, 0);
        assert!(!old.usable(0, 1));
        assert!(!saved(0, HOUR).usable(0, HOUR));
    }

    #[test]
    fn a_stale_save_is_not() {
        let s = saved(290, HOUR);
        assert!(s.usable(290, HOUR + FRESH_SECONDS * TICKS_PER_SECOND));
        assert!(!s.usable(290, HOUR + FRESH_SECONDS * TICKS_PER_SECOND + 1));
    }

    #[test]
    fn a_stamp_has_an_age_only_in_its_own_start_of_windows() {
        let at = Stamp {
            boot_id: 290,
            ticks: HOUR,
        };
        let later = |boot_id, ticks| Stamp { boot_id, ticks };
        assert_eq!(at.seconds_before(at), Some(0));
        assert_eq!(
            at.seconds_before(later(290, HOUR + 90 * TICKS_PER_SECOND)),
            Some(90)
        );
        assert_eq!(
            at.seconds_before(later(290, HOUR + 2 * TICKS_PER_SECOND - 1)),
            Some(1)
        );
        assert_eq!(at.seconds_before(later(291, HOUR + TICKS_PER_SECOND)), None);
        assert_eq!(at.seconds_before(later(290, HOUR - 1)), None);
        assert_eq!(Stamp::default().seconds_before(Stamp::default()), None);
        let none = Stamp {
            boot_id: 0,
            ticks: 1,
        };
        assert_eq!(none.seconds_before(later(0, HOUR)), None);
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
