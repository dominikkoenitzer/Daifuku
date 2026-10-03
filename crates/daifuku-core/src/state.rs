//! What each agent is doing, and what a terminal window shows as a result.
//!
//! An agent reports through its hooks: every hook event it fires arrives as a
//! [`HookEvent`] naming the session and the event. [`Agents`] turns that
//! stream into one [`AgentState`] per session and, because one terminal window
//! can hold several sessions in tabs, one state per window: the most urgent of
//! its sessions. A window with a session waiting for approval shows waiting,
//! whatever its other tabs are doing, because that is the one that needs you.

use std::collections::{BTreeMap, BTreeSet};

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    /// The agent's own session id, stable for the life of the session.
    pub session_id: String,
    /// The event name, `PreToolUse`, `Stop` and so on.
    pub hook_event_name: String,
    /// Set on `Notification` events: `permission_prompt`, `idle_prompt`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    /// Set when the event comes from a subagent rather than the session's
    /// main thread. Subagents run alongside it and ask for permission on
    /// their own, under the same session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Set on tool events: the tool's name, `Bash`, `apply_patch` and so on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Set on `SessionStart` events: how the session started, `startup`,
    /// `resume`, `compact` and so on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Set on `PreCompact` and `PostCompact` events: `manual` for a
    /// `/compact` you ran, `auto` for one the agent started itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// When the hook started, in 100 ns ticks since Windows started, stamped
    /// by `daifuku hook` itself. Agents run their hooks in the background and
    /// they may finish out of order; this puts them back in order. Windows'
    /// interrupt time, not the wall clock, so setting the clock back does not
    /// make every event after it look late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daifuku_at: Option<u64>,
}

/// What an event means for the session that sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The session is now in this state.
    To(AgentState),
    /// The session is over; forget it.
    End,
    /// One subagent of the session finished; whatever it waited for is no
    /// longer asked.
    ThreadEnd,
    /// The session has sat at its prompt for a while: its main thread is
    /// neither working nor asking anything. Claude Code sends no `Stop` for
    /// a turn you interrupt, so this is how such a turn ends. A failure
    /// stays, and so does a subagent's question.
    Idle,
    /// The event says nothing about the state (a config change, a file
    /// watcher, an event this version does not know).
    Ignore,
}

impl HookEvent {
    /// Reads the JSON a hook receives on standard input. It is read as it
    /// comes and only the fields above are kept, so an event that carries a
    /// whole file, as a tool event for a large edit does, is never held in
    /// memory whole.
    ///
    /// # Errors
    ///
    /// When the input cannot be read, is not JSON or lacks the session id or
    /// event name.
    pub fn from_hook_input(input: impl std::io::Read) -> Result<Self, serde_json::Error> {
        serde_json::from_reader(input)
    }

    /// What this event means.
    ///
    /// Claude Code's events, by what they tell us:
    ///
    /// - A prompt was sent, a tool is about to run or just ran, a subagent
    ///   started, the chat is being compacted: **working**. A tool that
    ///   failed is still working, because the agent carries on and tries
    ///   something else.
    /// - A permission prompt is up, an MCP server is asking a question, or
    ///   Codex asks one with its `request_user_input` tool: **waiting**. A
    ///   denied permission or an answer puts it back to work.
    /// - The turn ended, or a `/compact` you ran is finished: **done**. It
    ///   ended on an error: **failed**. A rate-limited session that resumed
    ///   by itself is working again. A session that starts again after a
    ///   compaction says nothing new: one the agent started itself may be in
    ///   the middle of a turn, and Codex starts it only with the next prompt.
    /// - Nothing has happened at the prompt for about a minute: see
    ///   [`Transition::Idle`].
    /// - The session closed: forget it.
    #[must_use]
    pub fn transition(&self) -> Transition {
        match self.hook_event_name.as_str() {
            // Codex has no event for its own questions: the tool that asks
            // one is the only sign, and it returns once you answer.
            "PreToolUse" if self.tool_name.as_deref() == Some("request_user_input") => {
                Transition::To(AgentState::Waiting)
            }
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
            | "PostToolBatch" | "PermissionDenied" | "SubagentStart" | "PreCompact"
            | "ElicitationResult" => Transition::To(AgentState::Working),
            "PostCompact" if self.trigger.as_deref() == Some("manual") => {
                Transition::To(AgentState::Done)
            }
            "PostCompact" => Transition::To(AgentState::Working),
            "PermissionRequest" | "Elicitation" => Transition::To(AgentState::Waiting),
            // Not `agent_needs_input`: that is a background session asking
            // while agent view is open. It runs apart from this window and
            // never reports from it, so nothing would clear the wait once
            // it is answered.
            "Notification" => match self.notification_type.as_deref() {
                Some("permission_prompt" | "elicitation_dialog" | "elicitation_url_dialog") => {
                    Transition::To(AgentState::Waiting)
                }
                Some("quota_auto_resume_fired") => Transition::To(AgentState::Working),
                Some("idle_prompt") => Transition::Idle,
                _ => Transition::Ignore,
            },
            "SessionStart" if self.source.as_deref() == Some("compact") => Transition::Ignore,
            // Codex reports a turn the user interrupted as an event of its
            // own; the agent is idle either way.
            "SessionStart" | "Stop" | "Interrupt" => Transition::To(AgentState::Done),
            "StopFailure" => Transition::To(AgentState::Failed),
            "SessionEnd" => Transition::End,
            "SubagentStop" => Transition::ThreadEnd,
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
    /// The latest hook time of sessions that ended, so a late event of one
    /// does not bring it back. Oldest first, at most [`ENDED`] of them.
    ended: std::collections::VecDeque<(String, u64)>,
    /// Bumped on every change, so "the oldest waiting window" is well defined
    /// without a clock.
    tick: u64,
}

#[derive(Debug, Clone)]
struct Session<W> {
    window: W,
    /// What the session's threads did last, apart from waiting.
    state: AgentState,
    /// The threads with a prompt up, by agent id; the main thread is "".
    /// A session waits while any of them does, whatever the others do in
    /// the meantime.
    waiting: BTreeSet<String>,
    /// The tick the session entered the state it shows.
    since: u64,
    /// The hook time of the latest event applied, when hooks stamp one.
    at: Option<u64>,
}

impl<W> Session<W> {
    /// The state the session shows.
    fn shown(&self) -> AgentState {
        if self.waiting.is_empty() {
            self.state
        } else {
            AgentState::Waiting
        }
    }
}

/// How many ended sessions [`Agents`] remembers to turn away their late
/// events.
const ENDED: usize = 256;

/// The longest session or subagent id [`Agents`] takes, in bytes. Far
/// longer than any agent's (Claude Code and Codex use 36-character UUIDs,
/// the demo `demo-<n>`).
const MAX_SESSION_ID: usize = 128;

/// The most threads one session can have waiting at once. Far more than
/// subagents run at a time; with [`MAX_SESSION_ID`] it bounds what made-up
/// events can make one tracked session cost to a few kilobytes.
const MAX_WAITING: usize = 32;

impl<W: Ord + Copy> Default for Agents<W> {
    fn default() -> Self {
        Self {
            sessions: BTreeMap::new(),
            ended: std::collections::VecDeque::new(),
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
    /// whether any window's state changed as a result. An event with a
    /// session or subagent id longer than any agent's is dropped.
    pub fn apply(&mut self, window: W, event: &HookEvent) -> bool {
        let too_long = event.session_id.len() > MAX_SESSION_ID
            || event
                .agent_id
                .as_ref()
                .is_some_and(|a| a.len() > MAX_SESSION_ID);
        if too_long || self.is_late(event) {
            return false;
        }
        let before = self.window_state(window);
        let moved_from = self.sessions.get(&event.session_id).map(|s| s.window);
        match event.transition() {
            Transition::Ignore => return false,
            Transition::End => {
                self.sessions.remove(&event.session_id);
                if let Some(at) = event.daifuku_at {
                    if self.ended.len() == ENDED {
                        self.ended.pop_front();
                    }
                    self.ended.push_back((event.session_id.clone(), at));
                }
            }
            Transition::ThreadEnd => {
                let (Some(id), Some(s)) =
                    (&event.agent_id, self.sessions.get_mut(&event.session_id))
                else {
                    return false;
                };
                let shown = s.shown();
                s.waiting.remove(id);
                s.at = s.at.max(event.daifuku_at);
                if s.shown() != shown {
                    self.tick += 1;
                    s.since = self.tick;
                }
            }
            Transition::Idle => {
                let Some(s) = self.sessions.get_mut(&event.session_id) else {
                    return false;
                };
                let shown = s.shown();
                s.waiting.remove("");
                if s.state == AgentState::Working {
                    s.state = AgentState::Done;
                }
                s.at = s.at.max(event.daifuku_at);
                if s.shown() != shown {
                    self.tick += 1;
                    s.since = self.tick;
                }
            }
            Transition::To(state) => {
                self.tick += 1;
                let tick = self.tick;
                let entry = self
                    .sessions
                    .entry(event.session_id.clone())
                    .or_insert(Session {
                        window,
                        state: AgentState::Working,
                        waiting: BTreeSet::new(),
                        since: tick,
                        at: None,
                    });
                let shown = entry.shown();
                let thread = event.agent_id.clone().unwrap_or_default();
                if state == AgentState::Waiting {
                    // A notification names no thread: it is the reminder of a
                    // prompt already counted, or the only sign of one.
                    let counted =
                        event.hook_event_name == "Notification" && !entry.waiting.is_empty();
                    if !counted && entry.waiting.len() < MAX_WAITING {
                        entry.waiting.insert(thread);
                    }
                } else {
                    entry.waiting.remove(&thread);
                    // A prompt typed on the main thread means none is up.
                    if thread.is_empty() && event.hook_event_name == "UserPromptSubmit" {
                        entry.waiting.clear();
                    }
                    entry.state = state;
                }
                if entry.shown() != shown || entry.window != window {
                    entry.since = tick;
                }
                entry.window = window;
                entry.at = entry.at.max(event.daifuku_at);
            }
        }
        let changed_here = self.window_state(window) != before;
        // A session that reports from a new window (a tab dragged out into a
        // window of its own) also changes the window it left.
        let changed_there = moved_from.is_some_and(|w| w != window);
        changed_here || changed_there
    }

    /// Whether an event was sent before one already applied for its session,
    /// or before its session ended: it arrived late and is old news.
    fn is_late(&self, event: &HookEvent) -> bool {
        let Some(at) = event.daifuku_at else {
            return false;
        };
        match self.sessions.get(&event.session_id) {
            Some(s) => s.at.is_some_and(|last| at < last),
            None => self
                .ended
                .iter()
                .any(|(id, end)| *id == event.session_id && at <= *end),
        }
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
            .map(Session::shown)
            .max()
    }

    /// Every window with at least one session, and the state it shows.
    #[must_use]
    pub fn windows(&self) -> BTreeMap<W, AgentState> {
        let mut out: BTreeMap<W, AgentState> = BTreeMap::new();
        for s in self.sessions.values() {
            out.entry(s.window)
                .and_modify(|e| *e = (*e).max(s.shown()))
                .or_insert(s.shown());
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
                    .filter(|s| s.window == window && s.shown() == state)
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

    /// Whether a session with this id is known.
    #[must_use]
    pub fn knows(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_makes_it_not_empty() {
        let mut a = Agents::new();
        assert!(a.is_empty());
        a.apply(1, &ev("a", "PreToolUse"));
        assert!(!a.is_empty());
    }

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

    #[test]
    fn knows_only_sessions_that_are_live() {
        let mut a = Agents::new();
        a.apply(1, &ev("a", "PreToolUse"));
        assert!(a.knows("a"));
        assert!(!a.knows("b"));
        a.apply(1, &ev("a", "SessionEnd"));
        assert!(!a.knows("a"));
    }

    fn ev(session: &str, name: &str) -> HookEvent {
        HookEvent {
            session_id: session.into(),
            hook_event_name: name.into(),
            ..HookEvent::default()
        }
    }

    fn at(event: HookEvent, at: u64) -> HookEvent {
        HookEvent {
            daifuku_at: Some(at),
            ..event
        }
    }

    fn sub(session: &str, name: &str, agent: &str) -> HookEvent {
        HookEvent {
            agent_id: Some(agent.into()),
            ..ev(session, name)
        }
    }

    #[test]
    fn a_subagent_waiting_stays_waiting_while_another_thread_works() {
        let mut a = Agents::new();
        a.apply(1, &sub("s", "PermissionRequest", "A"));
        assert!(!a.apply(1, &sub("s", "PreToolUse", "B")));
        assert!(!a.apply(1, &ev("s", "PostToolUse")));
        assert_eq!(a.window_state(1), Some(AgentState::Waiting));
        assert_eq!(a.needs_you(), vec![1]);
        // The thread that asked carries on: it was approved.
        a.apply(1, &sub("s", "PostToolUse", "A"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
    }

    #[test]
    fn a_subagent_that_ends_takes_its_question_with_it() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "UserPromptSubmit"));
        a.apply(1, &sub("s", "PermissionRequest", "A"));
        assert!(a.apply(1, &sub("s", "SubagentStop", "A")));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
        // A stop without a subagent says nothing.
        assert!(!a.apply(1, &ev("s", "SubagentStop")));
    }

    #[test]
    fn a_new_prompt_means_no_question_is_still_up() {
        let mut a = Agents::new();
        a.apply(1, &sub("s", "PermissionRequest", "A"));
        a.apply(1, &ev("s", "PermissionRequest"));
        a.apply(1, &ev("s", "UserPromptSubmit"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
    }

    #[test]
    fn a_reminder_counts_only_when_nothing_is_waiting_yet() {
        let mut a = Agents::new();
        a.apply(1, &sub("s", "PermissionRequest", "A"));
        a.apply(1, &note("s", "permission_prompt"));
        a.apply(1, &sub("s", "PostToolUse", "A"));
        assert_eq!(
            a.window_state(1),
            Some(AgentState::Working),
            "the reminder was A's"
        );
        a.apply(1, &note("s", "permission_prompt"));
        assert_eq!(a.window_state(1), Some(AgentState::Waiting));
    }

    #[test]
    fn a_late_event_does_not_undo_a_newer_one() {
        let mut a = Agents::new();
        a.apply(1, &at(ev("s", "PreToolUse"), 10));
        a.apply(1, &at(ev("s", "Stop"), 30));
        // The tool hook, started before the stop, finishes after it.
        assert!(!a.apply(1, &at(ev("s", "PostToolUse"), 20)));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
        // Unstamped events, from an older hook, are applied as they come.
        a.apply(1, &ev("s", "PostToolUse"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
    }

    #[test]
    fn a_late_event_does_not_bring_an_ended_session_back() {
        let mut a = Agents::new();
        a.apply(1, &at(ev("s", "PreToolUse"), 10));
        a.apply(1, &at(ev("s", "SessionEnd"), 30));
        assert!(!a.apply(1, &at(ev("s", "PostToolUse"), 20)));
        assert_eq!(a.window_state(1), None);
        // A session that starts again later is a new one.
        a.apply(1, &at(ev("s", "SessionStart"), 40));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
    }

    #[test]
    fn made_up_subagents_cannot_grow_a_session_without_end() {
        let mut a = Agents::new();
        let long = "x".repeat(MAX_SESSION_ID + 1);
        assert!(!a.apply(1, &sub("s", "PermissionRequest", &long)));
        for i in 0..(MAX_WAITING * 4) {
            a.apply(1, &sub("s", "PermissionRequest", &i.to_string()));
        }
        assert_eq!(a.sessions["s"].waiting.len(), MAX_WAITING);
    }

    #[test]
    fn a_session_id_no_agent_would_send_is_turned_away() {
        let long = "x".repeat(MAX_SESSION_ID + 1);
        let mut a = Agents::new();
        assert!(!a.apply(1, &ev(&long, "PermissionRequest")));
        assert!(a.is_empty());
        a.apply(1, &at(ev(&long, "SessionEnd"), 10));
        assert!(a.ended.is_empty());
        let longest = "x".repeat(MAX_SESSION_ID);
        assert!(a.apply(1, &ev(&longest, "PermissionRequest")));
    }

    #[test]
    fn a_rate_limit_that_resumes_by_itself_is_working_again() {
        assert_eq!(
            note("s", "quota_auto_resume_fired").transition(),
            Transition::To(AgentState::Working)
        );
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
        let e = HookEvent::from_hook_input(input.as_bytes()).unwrap();
        assert_eq!(e.session_id, "abc");
        assert_eq!(e.transition(), Transition::To(AgentState::Waiting));
    }

    #[test]
    fn reads_a_tool_event_that_carries_a_whole_file() {
        // An edit of a large file, its content more than a few megabytes.
        let content = "x".repeat(4 * 1024 * 1024);
        let input = format!(
            r#"{{"session_id":"abc","hook_event_name":"PermissionRequest",
            "tool_name":"Write","tool_input":{{"file_path":"C:\\x.json","content":"{content}"}}}}"#
        );
        let e = HookEvent::from_hook_input(input.as_bytes()).unwrap();
        assert_eq!(e.transition(), Transition::To(AgentState::Waiting));
    }

    #[test]
    fn input_without_a_session_is_rejected() {
        let missing = r#"{"hook_event_name":"Stop"}"#;
        assert!(HookEvent::from_hook_input(missing.as_bytes()).is_err());
        assert!(HookEvent::from_hook_input("not json".as_bytes()).is_err());
        // An event cut off part way is no event.
        let cut = r#"{"session_id":"abc","hook_event_name":"Stop","#;
        assert!(HookEvent::from_hook_input(cut.as_bytes()).is_err());
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

    fn compacted(name: &str, trigger: &str) -> HookEvent {
        HookEvent {
            trigger: Some(trigger.into()),
            ..ev("s", name)
        }
    }

    fn started(source: &str) -> HookEvent {
        HookEvent {
            source: Some(source.into()),
            ..ev("s", "SessionStart")
        }
    }

    #[test]
    fn a_compaction_in_the_middle_of_a_turn_keeps_it_working() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "UserPromptSubmit"));
        for e in [
            compacted("PreCompact", "auto"),
            compacted("PostCompact", "auto"),
            started("compact"),
        ] {
            a.apply(1, &e);
            let name = &e.hook_event_name;
            assert_eq!(a.window_state(1), Some(AgentState::Working), "{name}");
        }
    }

    #[test]
    fn a_compact_you_ran_ends_done() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "Stop"));
        a.apply(1, &compacted("PreCompact", "manual"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
        a.apply(1, &compacted("PostCompact", "manual"));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
        // Codex starts the compacted session just before the next model
        // request, so after the next prompt.
        a.apply(1, &ev("s", "UserPromptSubmit"));
        assert!(!a.apply(1, &started("compact")));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
        // Any other start is a session at its prompt.
        a.apply(1, &started("resume"));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
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
    fn a_turn_you_interrupt_is_done_once_the_prompt_sits_idle() {
        let mut a = Agents::new();
        // Esc in the middle of a turn: Claude Code sends no Stop.
        a.apply(1, &ev("s", "UserPromptSubmit"));
        assert!(a.apply(1, &note("s", "idle_prompt")));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
        // Esc, or no, at a permission prompt: nothing says so either.
        a.apply(1, &ev("s", "PreToolUse"));
        a.apply(1, &ev("s", "PermissionRequest"));
        assert!(a.apply(1, &note("s", "idle_prompt")));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
    }

    #[test]
    fn an_idle_prompt_keeps_failures_and_subagent_questions() {
        let mut a = Agents::new();
        a.apply(1, &ev("f", "StopFailure"));
        assert!(!a.apply(1, &note("f", "idle_prompt")));
        assert_eq!(a.window_state(1), Some(AgentState::Failed));
        // A subagent in the background may ask after the main turn is over.
        a.apply(2, &ev("s", "Stop"));
        a.apply(2, &sub("s", "PreToolUse", "A"));
        a.apply(2, &sub("s", "PermissionRequest", "A"));
        assert!(!a.apply(2, &note("s", "idle_prompt")));
        assert_eq!(a.window_state(2), Some(AgentState::Waiting));
        a.apply(2, &sub("s", "SubagentStop", "A"));
        assert_eq!(a.window_state(2), Some(AgentState::Done));
        // A session never seen is not one to start tracking now.
        assert!(!a.apply(3, &note("t", "idle_prompt")));
        assert!(!a.knows("t"));
    }

    #[test]
    fn every_kind_of_question_asked_in_the_window_means_waiting() {
        for kind in ["elicitation_dialog", "elicitation_url_dialog"] {
            let mut a = Agents::new();
            a.apply(1, &ev("s", "PreToolUse"));
            a.apply(1, &note("s", kind));
            assert_eq!(a.window_state(1), Some(AgentState::Waiting), "{kind}");
        }
    }

    #[test]
    fn a_background_session_asking_in_agent_view_is_not_this_windows_question() {
        let mut a = Agents::new();
        a.apply(1, &ev("s", "Stop"));
        assert!(!a.apply(1, &note("s", "agent_needs_input")));
        assert_eq!(a.window_state(1), Some(AgentState::Done));
    }

    #[test]
    fn a_codex_question_waits_until_it_is_answered() {
        let asking = |name: &str| HookEvent {
            tool_name: Some("request_user_input".into()),
            ..ev("s", name)
        };
        let mut a = Agents::new();
        a.apply(1, &ev("s", "UserPromptSubmit"));
        assert!(a.apply(1, &asking("PreToolUse")));
        assert_eq!(a.window_state(1), Some(AgentState::Waiting));
        a.apply(1, &asking("PostToolUse"));
        assert_eq!(a.window_state(1), Some(AgentState::Working));
        // Any other tool is work.
        let shell = HookEvent {
            tool_name: Some("Bash".into()),
            ..ev("s", "PreToolUse")
        };
        assert_eq!(shell.transition(), Transition::To(AgentState::Working));
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
