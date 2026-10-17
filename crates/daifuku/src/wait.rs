//! `daifuku wait`: blocks until an agent's terminal reaches a state, for a
//! script or an agent that hands out work and wants to know when it is done.
//!
//! It asks the daemon for its status once a second, so it needs what every
//! command that talks to the daemon needs: an administrator terminal.

use daifuku_core::protocol::Status;
use daifuku_core::state::AgentState;
use serde::Serialize;

/// The state to wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Until {
    /// Anything but working: done, waiting for you or failed.
    Stopped,
    /// Finished its turn.
    Done,
    /// Waiting for you.
    Waiting,
    /// The turn ended on an error.
    Failed,
}

impl Until {
    /// Whether `state` is what is waited for.
    #[must_use]
    pub fn reached(self, state: AgentState) -> bool {
        match self {
            Self::Stopped => state != AgentState::Working,
            Self::Done => state == AgentState::Done,
            Self::Waiting => state == AgentState::Waiting,
            Self::Failed => state == AgentState::Failed,
        }
    }
}

/// What `daifuku wait --json` prints once the state is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Waited {
    /// Always `waited`.
    pub result: &'static str,
    /// The terminal's window.
    pub window: u64,
    /// The state it reached, or `closed` when its terminal closed.
    pub state: &'static str,
    /// Its label, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The session whose state it shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// How long the wait took, in seconds.
    pub seconds: u64,
}

/// Where the terminal waited for is, as one status shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// It reached the state: its window, the state, its label and session.
    Reached(u64, AgentState, Option<String>, Option<String>),
    /// Not yet; its window, once known.
    Waiting(Option<u64>),
    /// The terminal it was is gone.
    Closed(u64),
    /// No terminal is called that.
    Unknown,
}

/// A window handle as typed: decimal, as `status --json` writes it, or hex
/// with `0x`, as `status` prints it.
#[must_use]
pub fn window_number(text: &str) -> Option<u64> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Where `target`, a window handle or a label in any case, is in `status`,
/// `seen` being its window when an earlier status found it.
#[must_use]
pub fn step(status: &Status, target: &str, until: Until, seen: Option<u64>) -> Step {
    let number = window_number(target);
    // Once found, it is followed by its window: a label can move on to
    // another terminal, a window cannot.
    let is = |window: u64, label: Option<&String>| match seen {
        Some(w) => window == w,
        None => number == Some(window) || label.is_some_and(|l| l.eq_ignore_ascii_case(target)),
    };
    let agent = status
        .agents
        .iter()
        .find(|a| is(a.window, a.label.as_ref()));
    if let Some(a) = agent {
        if until.reached(a.state) {
            return Step::Reached(a.window, a.state, a.label.clone(), a.session_id.clone());
        }
        return Step::Waiting(Some(a.window));
    }
    // Opened, but its agent has not reported yet.
    let peer = status
        .peers
        .iter()
        .find(|p| is(p.window, p.label.as_ref()))
        .map(|p| p.window);
    let in_fleet = status
        .fleets
        .iter()
        .flat_map(|f| &f.windows)
        .find(|&&w| is(w, None))
        .copied();
    match (peer.or(in_fleet), seen) {
        (Some(w), _) => Step::Waiting(Some(w)),
        (None, Some(w)) => Step::Closed(w),
        (None, None) => Step::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daifuku_core::protocol::{AgentWindow, FleetStatus, PeerStatus};

    fn agent(window: u64, state: AgentState, label: Option<&str>) -> AgentWindow {
        AgentWindow {
            window,
            title: String::new(),
            state,
            for_seconds: 0,
            label: label.map(Into::into),
            fleet: None,
            session_id: Some(format!("s{window}")),
        }
    }

    fn status() -> Status {
        Status {
            agents: vec![
                agent(10, AgentState::Working, Some("Site QA")),
                agent(11, AgentState::Done, Some("agents 2")),
                agent(12, AgentState::Waiting, None),
            ],
            peers: vec![PeerStatus {
                window: 20,
                label: Some("docs".into()),
                directory: r"C:\src\docs".into(),
                session_id: None,
            }],
            fleets: vec![FleetStatus {
                name: "agents".into(),
                monitor: String::new(),
                windows: vec![11, 30],
            }],
            ..Status::default()
        }
    }

    #[test]
    fn a_window_is_typed_in_decimal_or_hex() {
        assert_eq!(window_number("1050190"), Some(1_050_190));
        assert_eq!(window_number("0x0010064e"), Some(0x0010_064e));
        assert_eq!(window_number("0X1F"), Some(31));
        assert_eq!(window_number("site"), None);
        assert_eq!(window_number("0xzz"), None);
    }

    #[test]
    fn stopped_is_anything_but_working() {
        assert!(!Until::Stopped.reached(AgentState::Working));
        for s in [AgentState::Done, AgentState::Waiting, AgentState::Failed] {
            assert!(Until::Stopped.reached(s));
        }
        assert!(Until::Done.reached(AgentState::Done));
        assert!(!Until::Done.reached(AgentState::Waiting));
        assert!(Until::Failed.reached(AgentState::Failed));
        assert!(Until::Waiting.reached(AgentState::Waiting));
    }

    #[test]
    fn a_terminal_is_found_by_its_window_or_its_label_in_any_case() {
        let s = status();
        assert_eq!(
            step(&s, "site qa", Until::Done, None),
            Step::Waiting(Some(10))
        );
        assert_eq!(
            step(&s, "AGENTS 2", Until::Done, None),
            Step::Reached(
                11,
                AgentState::Done,
                Some("agents 2".into()),
                Some("s11".into())
            )
        );
        assert_eq!(
            step(&s, "12", Until::Stopped, None),
            Step::Reached(12, AgentState::Waiting, None, Some("s12".into()))
        );
        assert_eq!(step(&s, "0xc", Until::Done, None), Step::Waiting(Some(12)));
    }

    #[test]
    fn a_terminal_whose_agent_has_not_reported_yet_is_waited_for() {
        let s = status();
        assert_eq!(step(&s, "docs", Until::Done, None), Step::Waiting(Some(20)));
        assert_eq!(step(&s, "30", Until::Done, None), Step::Waiting(Some(30)));
    }

    #[test]
    fn a_terminal_that_was_there_and_went_is_closed_and_a_new_name_unknown() {
        let s = status();
        assert_eq!(step(&s, "Site QA", Until::Done, Some(99)), Step::Closed(99));
        assert_eq!(step(&s, "nobody", Until::Done, None), Step::Unknown);
        assert_eq!(step(&s, "77", Until::Done, None), Step::Unknown);
    }
}
