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

