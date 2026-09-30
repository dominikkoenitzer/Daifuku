//! What each agent is doing, and what a terminal window shows as a result.
//!
//! An agent reports through its hooks: every hook event it fires arrives as a
//! [`HookEvent`] naming the session and the event. [`Agents`] turns that
//! stream into one [`AgentState`] per session and, because one terminal window
//! can hold several sessions in tabs, one state per window: the most urgent of
//! its sessions. A window with a session waiting for approval shows waiting,
//! whatever its other tabs are doing, because that is the one that needs you.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What an agent is doing right now.
///
/// The order is urgency, least to most: a window takes the maximum over its
/// sessions, so [`AgentState::Waiting`] beats everything.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Finished its turn, or never started one: nothing to do until you type.
    Done,
    /// Thinking, calling tools, writing.
    Working,
    /// The turn ended on an error: rate limit, authentication, an API fault.
    Failed,
    /// Blocked on you: a permission prompt or a question it asked.
    Waiting,
}

impl AgentState {
    /// Every state, least urgent first.
    pub const ALL: [Self; 4] = [Self::Done, Self::Working, Self::Failed, Self::Waiting];

    /// The lower-case name used in the config file and on the command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Working => "working",
            Self::Failed => "failed",
            Self::Waiting => "waiting",
        }
    }
}

/// One hook call as the agent reported it: the fields Daifuku reads from the
/// JSON a hook receives on standard input. Everything else in that JSON is
/// ignored, so a newer agent adding fields changes nothing here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    /// The agent's own session id, stable for the life of the session.
    pub session_id: String,
    /// The event name, `PreToolUse`, `Stop` and so on.
    pub hook_event_name: String,
    /// Set on `Notification` events: `permission_prompt`, `idle_prompt`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
}

/// What an event means for the session that sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The session is now in this state.
    To(AgentState),
    /// The session is over; forget it.
    End,
    /// The event says nothing about the state (a config change, a file
    /// watcher, an event this version does not know).
    Ignore,
}

impl HookEvent {
    /// Reads the JSON a hook receives on standard input.
    ///
    /// # Errors
    ///
    /// When the input is not JSON or lacks the session id or event name.
    pub fn from_hook_input(input: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(input)
    }

    /// What this event means.
    ///
    /// Claude Code's events, by what they tell us:
    ///
    /// - A prompt was sent, a tool is about to run or just ran, a subagent
    ///   started: **working**. A tool that failed is still working, because
    ///   the agent carries on and tries something else.
    /// - A permission prompt is up, or an MCP server is asking a question:
    ///   **waiting**. A denied permission puts it back to work.
    /// - The turn ended: **done**. It ended on an error: **failed**.
    /// - The session closed: forget it.
    #[must_use]
    pub fn transition(&self) -> Transition {
        match self.hook_event_name.as_str() {
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
            | "PostToolBatch" | "PermissionDenied" | "SubagentStart" | "PreCompact"
            | "PostCompact" | "ElicitationResult" => Transition::To(AgentState::Working),
            "PermissionRequest" | "Elicitation" => Transition::To(AgentState::Waiting),
            "Notification" => match self.notification_type.as_deref() {
                Some(
                    "permission_prompt"
                    | "elicitation_dialog"
                    | "elicitation_url_dialog"
                    | "agent_needs_input",
                ) => Transition::To(AgentState::Waiting),
                _ => Transition::Ignore,
            },
            // Codex reports a turn the user interrupted as an event of its
            // own; the agent is idle either way.
            "SessionStart" | "Stop" | "Interrupt" => Transition::To(AgentState::Done),
            "StopFailure" => Transition::To(AgentState::Failed),
            "SessionEnd" => Transition::End,
            _ => Transition::Ignore,
        }
    }
}

/// Every known session, grouped by the window it runs in.
///
/// The window key is opaque here: the daemon uses the window handle. Keeping
/// it generic is what lets every rule below be tested without a desktop.
#[derive(Debug, Clone)]
pub struct Agents<W: Ord + Copy> {
    sessions: BTreeMap<String, Session<W>>,
    /// Bumped on every change, so "the oldest waiting window" is well defined
    /// without a clock.
    tick: u64,
}

#[derive(Debug, Clone, Copy)]
struct Session<W> {
    window: W,
    state: AgentState,
    /// The tick the session entered its current state.
    since: u64,
}

impl<W: Ord + Copy> Default for Agents<W> {
    fn default() -> Self {
        Self {
            sessions: BTreeMap::new(),
            tick: 0,
        }
    }
}

impl<W: Ord + Copy> Agents<W> {
    /// No sessions yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one hook event from a session running in `window`, and says
    /// whether any window's state changed as a result.
    pub fn apply(&mut self, window: W, event: &HookEvent) -> bool {
        let before = self.window_state(window);
        let moved_from = self.sessions.get(&event.session_id).map(|s| s.window);
        match event.transition() {
            Transition::Ignore => return false,
            Transition::End => {
                self.sessions.remove(&event.session_id);
            }
            Transition::To(state) => {
                self.tick += 1;
                let tick = self.tick;
                let entry = self
                    .sessions
                    .entry(event.session_id.clone())
                    .or_insert(Session {
                        window,
                        state,
                        since: tick,
                    });
                if entry.state != state || entry.window != window {
                    entry.since = tick;
                }
                entry.window = window;
                entry.state = state;
            }
        }
        let changed_here = self.window_state(window) != before;
        // A session that reports from a new window (a tab dragged out into a
        // window of its own) also changes the window it left.
        let changed_there = moved_from.is_some_and(|w| w != window);
        changed_here || changed_there
    }

    /// Forgets every session in a window that no longer exists. Windows reuse
    /// handles, so a closed window's sessions must go the moment it dies or a
    /// new, unrelated window inherits its colour.
    pub fn forget_window(&mut self, window: W) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|_, s| s.window != window);
        self.sessions.len() != before
    }

    /// The state a window shows: the most urgent of its sessions, or `None`
    /// when no agent has ever reported from it.
    #[must_use]
    pub fn window_state(&self, window: W) -> Option<AgentState> {
        self.sessions
            .values()
            .filter(|s| s.window == window)
            .map(|s| s.state)
            .max()
    }

    /// Every window with at least one session, and the state it shows.
    #[must_use]
    pub fn windows(&self) -> BTreeMap<W, AgentState> {
        let mut out: BTreeMap<W, AgentState> = BTreeMap::new();
        for s in self.sessions.values() {
            out.entry(s.window)
                .and_modify(|e| *e = (*e).max(s.state))
                .or_insert(s.state);
        }
        out
    }

    /// The windows that need you, the one that has waited longest first:
    /// waiting before failed, and within each, oldest first.
    #[must_use]
    pub fn needs_you(&self) -> Vec<W> {
        let mut hits: Vec<(AgentState, u64, W)> = Vec::new();
        for (window, state) in self.windows() {
            if matches!(state, AgentState::Waiting | AgentState::Failed) {
                let since = self
                    .sessions
                    .values()
                    .filter(|s| s.window == window && s.state == state)
                    .map(|s| s.since)
                    .min()
                    .unwrap_or(0);
                hits.push((state, since, window));
            }
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.into_iter().map(|(_, _, w)| w).collect()
    }

    /// How many sessions are known.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether no session is known.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_again_goes_to_the_back_of_the_queue() {
        let mut a = Agents::new();
        a.apply(1, &ev("a", "PermissionRequest"));
        a.apply(2, &ev("b", "PermissionRequest"));
        a.apply(1, &ev("a", "PostToolUse"));
        a.apply(1, &ev("a", "PermissionRequest"));
        assert_eq!(a.needs_you(), vec![2, 1]);
    }

    #[test]
    fn a_session_that_moves_window_starts_waiting_anew() {
        let mut a = Agents::new();
        a.apply(1, &ev("a", "PermissionRequest"));
        a.apply(2, &ev("b", "PermissionRequest"));
        a.apply(3, &ev("a", "PermissionRequest"));
        assert_eq!(a.needs_you(), vec![2, 3]);
    }

    #[test]
    fn leaving_a_window_is_a_change_even_where_it_lands_looks_the_same() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "PermissionRequest"));
        a.apply(2, &ev("t", "PermissionRequest"));
        assert!(
            a.apply(2, &ev("s", "PermissionRequest")),
            "window 1 lost its session"
        );
        assert_eq!(a.window_state(1), None);
    }

    #[test]
    fn a_windows_wait_is_timed_from_its_waiting_sessions_only() {
        let mut a = Agents::new();
        a.apply(1, &ev("old-failure", "StopFailure"));
        a.apply(2, &ev("c", "PermissionRequest"));
        a.apply(1, &ev("b", "PermissionRequest"));
        // Window 1 has waited since after window 2 did; its older failure
        // does not count towards how long it has been waiting.
        assert_eq!(a.needs_you(), vec![2, 1]);
    }

    #[test]
    fn len_counts_sessions() {
        let mut a = Agents::new();
        assert_eq!(a.len(), 0);
        a.apply(1, &ev("a", "PreToolUse"));
        a.apply(1, &ev("b", "PreToolUse"));
        a.apply(2, &ev("c", "PreToolUse"));
        assert_eq!(a.len(), 3);
    }

    fn ev(session: &str, name: &str) -> HookEvent {
        HookEvent {
            session_id: session.into(),
            hook_event_name: name.into(),
            notification_type: None,
        }
    }

    fn note(session: &str, kind: &str) -> HookEvent {
        HookEvent {
            notification_type: Some(kind.into()),
            ..ev(session, "Notification")
        }
    }

    #[test]
    fn parses_real_hook_input_and_ignores_unknown_fields() {
        let input = r#"{"session_id":"abc","prompt_id":"p","hook_event_name":"Notification",
            "cwd":"C:\\x","transcript_path":"t","permission_mode":"default",
            "notification_type":"permission_prompt","message":"Claude needs your permission"}"#;
        let e = HookEvent::from_hook_input(input).unwrap();
        assert_eq!(e.session_id, "abc");
        assert_eq!(e.transition(), Transition::To(AgentState::Waiting));
    }

    #[test]
    fn input_without_a_session_is_rejected() {
        assert!(HookEvent::from_hook_input(r#"{"hook_event_name":"Stop"}"#).is_err());
        assert!(HookEvent::from_hook_input("not json").is_err());
    }

    #[test]
    fn a_turn_goes_working_waiting_working_done() {
        let mut a = Agents::new();
        for (name, want) in [
            ("SessionStart", AgentState::Done),
            ("UserPromptSubmit", AgentState::Working),
            ("PermissionRequest", AgentState::Waiting),
            ("PostToolUse", AgentState::Working),
            ("Stop", AgentState::Done),
        ] {
            a.apply(1, &ev("s", name));
            assert_eq!(a.window_state(1), Some(want), "after {name}");
        }
    }

    #[test]
    fn a_failed_turn_is_failed_until_the_next_prompt() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "UserPromptSubmit"));
        a.apply(1, &ev("s", "StopFailure"));
        assert_eq!(a.window_state(1), Some(AgentState::Failed));
        a.apply(1, &ev("s", "UserPromptSubmit"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
    }

    #[test]
    fn a_failing_tool_does_not_fail_the_agent() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "PostToolUseFailure"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
    }

    #[test]
    fn only_permission_notifications_mean_waiting() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "Stop"));
        assert!(!a.apply(1, &note("s", "idle_prompt")));
        assert!(!a.apply(1, &note("s", "auth_success")));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
        a.apply(1, &note("s", "permission_prompt"));
        assert_eq!(a.window_state(1), Some(AgentState::Waiting));
    }

    #[test]
    fn every_documented_kind_of_question_means_waiting() {
        for kind in [
            "elicitation_dialog",
            "elicitation_url_dialog",
            "agent_needs_input",
        ] {
            let mut a = Agents::new();
            a.apply(1, &ev("s", "PreToolUse"));
            a.apply(1, &note("s", kind));
            assert_eq!(a.window_state(1), Some(AgentState::Waiting), "{kind}");
        }
    }

    #[test]
    fn an_answered_question_puts_the_agent_back_to_work() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "Elicitation"));
        a.apply(1, &ev("s", "ElicitationResult"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
    }

    #[test]
    fn unknown_events_change_nothing() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "PreToolUse"));
        assert!(!a.apply(1, &ev("s", "SomethingFromTheFuture")));
        assert!(!a.apply(1, &ev("s", "FileChanged")));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
    }

    #[test]
    fn a_window_shows_its_most_urgent_tab() {
        let mut a = Agents::new();
        a.apply(1, &ev("tab-a", "PreToolUse"));
        a.apply(1, &ev("tab-b", "Stop"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
        a.apply(1, &ev("tab-b", "PermissionRequest"));
        assert_eq!(a.window_state(1), Some(AgentState::Waiting));
    }

    #[test]
    fn session_end_forgets_the_session() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "PreToolUse"));
        assert!(a.apply(1, &ev("s", "SessionEnd")));
        assert_eq!(a.window_state(1), None);
        assert!(a.is_empty());
    }

    #[test]
    fn a_dead_window_takes_its_sessions_with_it() {
        let mut a = Agents::new();
        a.apply(1, &ev("x", "PermissionRequest"));
        a.apply(2, &ev("y", "PreToolUse"));
        assert!(a.forget_window(1));
        assert_eq!(a.window_state(1), None);
        assert_eq!(a.len(), 1);
        assert!(!a.forget_window(1));
    }

    #[test]
    fn a_tab_dragged_to_another_window_moves_its_state() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "PermissionRequest"));
        assert!(a.apply(2, &ev("s", "PermissionRequest")));
        assert_eq!(a.window_state(1), None);
        assert_eq!(a.window_state(2), Some(AgentState::Waiting));
    }

    #[test]
    fn needs_you_is_waiting_first_then_oldest_first() {
        let mut a = Agents::new();
        a.apply(1, &ev("a", "StopFailure"));
        a.apply(2, &ev("b", "PermissionRequest"));
        a.apply(3, &ev("c", "PreToolUse"));
        a.apply(4, &ev("d", "PermissionRequest"));
        a.apply(5, &ev("e", "Stop"));
        assert_eq!(a.needs_you(), vec![2, 4, 1]);
    }

    #[test]
    fn repeating_a_state_keeps_its_place_in_the_queue() {
        let mut a = Agents::new();
        a.apply(1, &ev("a", "PermissionRequest"));
        a.apply(2, &ev("b", "PermissionRequest"));
        a.apply(1, &note("a", "permission_prompt"));
        assert_eq!(a.needs_you(), vec![1, 2]);
    }

    #[test]
    fn state_names_round_trip_through_json() {
        for s in AgentState::ALL {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s.name()));
            assert_eq!(serde_json::from_str::<AgentState>(&json).unwrap(), s);
        }
    }
}
