//! `daifuku wait`: blocks until an agent's terminal reaches a state, for a
//! script or an agent that hands out work and wants to know when it is done.
//!
//! It asks the daemon for its status twice a second, so it needs what every
//! command that talks to the daemon needs: an administrator terminal.

use daifuku_core::protocol::Status;
use daifuku_core::state::AgentState;
use serde::Serialize;

/// The state to wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Until {
    /// The end of the agent's next turn: once it has been seen working,
    /// anything but working. A terminal that just opened shows done before
    /// its first task, which this does not take for the task's end.
    Turn,
    /// Anything but working, now: done, waiting for you or failed.
    Stopped,
    /// Finished its turn.
    Done,
    /// Waiting for you.
    Waiting,
    /// The turn ended on an error.
    Failed,
}

impl Until {
    /// Whether `state` is what is waited for, `worked` saying whether the
    /// agent has been seen working since the wait began.
    #[must_use]
    pub fn reached(self, state: AgentState, worked: bool) -> bool {
        match self {
            Self::Turn => worked && state != AgentState::Working,
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
    /// The state it reached; `exited` when its agent ended and the terminal
    /// stayed, `closed` when the terminal closed.
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
    /// Not yet.
    Waiting,
    /// Its agent ended, and the terminal is still open.
    Exited(u64),
    /// The terminal it was is gone.
    Closed(u64),
    /// No terminal is called that.
    Unknown,
}

/// What a wait has seen of its terminal so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Seen {
    /// Its window, once found.
    pub window: Option<u64>,
    /// Whether an agent has reported from it.
    pub agent: bool,
    /// Whether that agent has been seen working.
    pub worked: bool,
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
/// with `seen` what earlier statuses showed of it, brought up to date.
#[must_use]
pub fn step(status: &Status, target: &str, until: Until, seen: &mut Seen) -> Step {
    let number = window_number(target);
    // Once found, it is followed by its window: a label can move on to
    // another terminal, a window cannot.
    let followed = seen.window;
    let is = |window: u64, label: Option<&String>| match followed {
        Some(w) => window == w,
        None => number == Some(window) || label.is_some_and(|l| l.eq_ignore_ascii_case(target)),
    };
    let agent = status
        .agents
        .iter()
        .find(|a| is(a.window, a.label.as_ref()));
    if let Some(a) = agent {
        seen.window = Some(a.window);
        seen.agent = true;
        seen.worked |= a.state == AgentState::Working;
        if until.reached(a.state, seen.worked) {
            return Step::Reached(a.window, a.state, a.label.clone(), a.session_id.clone());
        }
        return Step::Waiting;
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
    match (peer.or(in_fleet), followed) {
        // Its agent reported and is gone: it ended, by `/exit` or because
        // its process did.
        (Some(w), _) if seen.agent => Step::Exited(w),
        (Some(w), _) => {
            seen.window = Some(w);
            Step::Waiting
        }
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
    fn stopped_is_anything_but_working_and_a_turn_needs_work_first() {
        assert!(!Until::Stopped.reached(AgentState::Working, false));
        for st in [AgentState::Done, AgentState::Waiting, AgentState::Failed] {
            assert!(Until::Stopped.reached(st, false));
            assert!(Until::Turn.reached(st, true));
            // Done right after a start is not the end of a turn.
            assert!(!Until::Turn.reached(st, false));
        }
        assert!(!Until::Turn.reached(AgentState::Working, true));
        assert!(Until::Done.reached(AgentState::Done, false));
        assert!(!Until::Done.reached(AgentState::Waiting, false));
        assert!(Until::Failed.reached(AgentState::Failed, false));
        assert!(Until::Waiting.reached(AgentState::Waiting, false));
    }

    fn first(s: &Status, target: &str, until: Until) -> (Step, Seen) {
        let mut seen = Seen::default();
        (step(s, target, until, &mut seen), seen)
    }

    #[test]
    fn a_terminal_is_found_by_its_window_or_its_label_in_any_case() {
        let s = status();
        let (found, seen) = first(&s, "site qa", Until::Done);
        assert_eq!(found, Step::Waiting);
        assert_eq!(seen.window, Some(10));
        assert_eq!(
            first(&s, "AGENTS 2", Until::Done).0,
            Step::Reached(
                11,
                AgentState::Done,
                Some("agents 2".into()),
                Some("s11".into())
            )
        );
        assert_eq!(
            first(&s, "12", Until::Stopped).0,
            Step::Reached(12, AgentState::Waiting, None, Some("s12".into()))
        );
        assert_eq!(first(&s, "0xc", Until::Done).1.window, Some(12));
    }

    #[test]
    fn a_turn_ends_only_after_the_agent_was_seen_working() {
        let mut s = status();
        let mut seen = Seen::default();
        // Done from its start: not yet.
        assert_eq!(step(&s, "agents 2", Until::Turn, &mut seen), Step::Waiting);
        s.agents[1].state = AgentState::Working;
        assert_eq!(step(&s, "agents 2", Until::Turn, &mut seen), Step::Waiting);
        assert!(seen.worked);
        s.agents[1].state = AgentState::Done;
        assert!(matches!(
            step(&s, "agents 2", Until::Turn, &mut seen),
            Step::Reached(11, AgentState::Done, ..)
        ));
    }

    #[test]
    fn a_terminal_whose_agent_has_not_reported_yet_is_waited_for() {
        let s = status();
        let (found, seen) = first(&s, "docs", Until::Done);
        assert_eq!((found, seen.window), (Step::Waiting, Some(20)));
        assert_eq!(first(&s, "30", Until::Done).1.window, Some(30));
    }

    #[test]
    fn an_agent_that_ended_in_a_terminal_that_stays_open_has_exited() {
        let mut s = status();
        let mut seen = Seen::default();
        assert_eq!(step(&s, "11", Until::Turn, &mut seen), Step::Waiting);
        // Its session forgotten, its terminal still in its fleet.
        s.agents.retain(|a| a.window != 11);
        assert_eq!(step(&s, "11", Until::Turn, &mut seen), Step::Exited(11));
    }

    #[test]
    fn a_terminal_that_was_there_and_went_is_closed_and_a_new_name_unknown() {
        let s = status();
        let mut seen = Seen {
            window: Some(99),
            ..Seen::default()
        };
        assert_eq!(
            step(&s, "Site QA", Until::Done, &mut seen),
            Step::Closed(99)
        );
        assert_eq!(first(&s, "nobody", Until::Done).0, Step::Unknown);
        assert_eq!(first(&s, "77", Until::Done).0, Step::Unknown);
    }
}
