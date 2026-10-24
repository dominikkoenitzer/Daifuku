//! What travels over Daifuku's two pipes, one JSON object per line.
//!
//! There are two pipes because there are two kinds of caller:
//!
//! - **The hook pipe** takes [`HookLine`] from agent hooks and status lines,
//!   which run at whatever integrity the agent runs at. The worst a message
//!   can do is colour a border or show a wrong number in `daifuku status`,
//!   so any process of the signed-in user may write to it.
//! - **The control pipe** takes [`Request`] from the command line. A request
//!   can open a fleet of administrator terminals when the daemon runs
//!   elevated, so then only an elevated process may connect; letting an
//!   ordinary process drive it would hand that process administrator rights.
//!   A daemon with the user's own rights lets in that user alone.

use serde::{Deserialize, Serialize};

use crate::peer::Spawn;
use crate::state::{AgentState, HookEvent};
use crate::usage::Limits;

/// One hook call, as the hook sends it to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookMessage {
    /// The top-level window the agent's terminal is, as a raw handle. The
    /// hook works this out itself: it runs inside the agent's process tree,
    /// where the answer is one `AttachConsole` away.
    pub window: u64,
    /// What the agent reported.
    pub event: HookEvent,
    /// The agent's own process: the one whose console led the hook to the
    /// window. When it ends, so has the agent, even when its terminal stays
    /// open. Left out by hooks older than this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// What an agent's status line reports: how much of its limits the account
/// it runs on has used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    /// Both windows, as Claude Code last told the status line.
    pub usage: Limits,
    /// The folder of the account the agent runs on, as it was given in
    /// `CLAUDE_CONFIG_DIR`, else the `.claude` folder in the profile.
    pub folder: String,
    /// The agent's terminal window, when the status line could find it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
}

/// One line on the hook pipe: an agent's event, or its account's usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HookLine {
    /// From a hook.
    Event(HookMessage),
    /// From a status line.
    Usage(UsageReport),
}

/// A command for the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Request {
    /// Open a fleet, the first one in the config when no name is given.
    Open {
        /// Fleet name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fleet: Option<String>,
    },
    /// Put every fleet terminal back in its cell.
    Snap,
    /// Close a fleet's terminals, the first fleet when no name is given.
    Close {
        /// Fleet name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fleet: Option<String>,
    },
    /// Close the terminals of every open fleet.
    CloseAll,
    /// Close one terminal Daifuku knows, a fleet's, a peer or one an agent
    /// reports from, by its window or its label.
    CloseTerminal {
        /// Its window handle, in decimal or as 0x and hex, or its label.
        terminal: String,
    },
    /// Focus the terminal that has waited longest.
    Next,
    /// Open six scripted demo agents, to see Daifuku without a real one.
    Demo,
    /// Open one agent terminal on its own, in the folder it works in.
    Spawn(Spawn),
    /// Give an agent's terminal a label, or take it away.
    Label {
        /// The terminal's window.
        window: u64,
        /// The label; `None` takes it away.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Open again the peers that were open when Windows last stopped, each
    /// resuming its last session in its own folder.
    Restore {
        /// Only say which peers it would open.
        #[serde(default)]
        dry_run: bool,
        /// Forget them instead.
        #[serde(default)]
        forget: bool,
    },
    /// Stop every agent at once: close every fleet's terminals and every
    /// peer, keep each session for `daifuku restore`, and end what the
    /// agents started that outlives its terminal.
    StopAgents {
        /// Only say what it would close, keep and end.
        #[serde(default)]
        dry_run: bool,
    },
    /// What the daemon knows.
    Status,
    /// Re-read the config file.
    Reload,
    /// Take the borders down and exit.
    Stop,
}

/// The daemon's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum Response {
    /// Done.
    Ok {
        /// A line for a person, if there is anything to say.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// The answer to [`Request::Status`].
    Status(Status),
    /// The answer to [`Request::Restore`]: every peer it looked at.
    Restored {
        /// Each peer and what became of it.
        peers: Vec<RestoredPeer>,
    },
    /// The answer to [`Request::StopAgents`].
    Stopped {
        /// Whether it only said what it would do.
        #[serde(default)]
        dry_run: bool,
        /// Each terminal it closed, or would close.
        terminals: Vec<StoppedTerminal>,
        /// Each process an agent started that it ended, or would end if it
        /// were still running once its terminal went.
        leftovers: Vec<Leftover>,
    },
    /// The answer to [`Request::Spawn`]: the terminal that opened.
    Spawned {
        /// Its window handle.
        window: u64,
        /// Its label, when it was given one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// The fleet whose grid it took a cell of, when it asked for one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fleet: Option<String>,
        /// That cell, counted from 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cell: Option<u32>,
    },
    /// It did not work.
    Error {
        /// Why, for a person.
        message: String,
    },
}

impl Response {
    /// A plain success.
    #[must_use]
    pub const fn ok() -> Self {
        Self::Ok { message: None }
    }

    /// A success with something to say.
    #[must_use]
    pub fn said(message: impl Into<String>) -> Self {
        Self::Ok {
            message: Some(message.into()),
        }
    }

    /// A failure.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

/// Everything `daifuku status` prints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The daemon's version.
    pub version: String,
    /// Where the config was read from.
    pub config: String,
    /// Whether the daemon runs elevated, which administrator fleets need.
    #[serde(default)]
    pub elevated: bool,
    /// Every hotkey and what it does, refused ones included.
    #[serde(default)]
    pub hotkeys: Vec<String>,
    /// Every open fleet.
    pub fleets: Vec<FleetStatus>,
    /// Every window an agent has reported from.
    pub agents: Vec<AgentWindow>,
    /// Every terminal `daifuku spawn` opened that is still open.
    #[serde(default)]
    pub peers: Vec<PeerStatus>,
    /// Each account's use of its limits, as its agents' status lines last
    /// reported it.
    #[serde(default)]
    pub usage: Vec<AccountUsage>,
}

/// One account's use of its limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUsage {
    /// Its name under `accounts`; left out for the account in the profile
    /// and for a folder no account names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Its folder.
    pub folder: String,
    /// How long ago a status line last reported it, in seconds.
    pub age_seconds: u64,
    /// The 5-hour window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<WindowUsage>,
    /// The weekly window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<WindowUsage>,
    /// The account's `warn`, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn: Option<u8>,
    /// Whether a window has reached `warn`, so the borders of the account's
    /// terminals show it.
    #[serde(default)]
    pub warning: bool,
}

/// One window of an account's limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowUsage {
    /// The share used, in percent, when it was reported.
    pub used: u8,
    /// When the window starts over, in seconds since 1970.
    pub resets_at: u64,
    /// How long until then, in seconds; left out once it has, when `used`
    /// no longer holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_in_seconds: Option<u64>,
    /// How far use is ahead of an even spread over the window, in
    /// percentage points; negative is to spare. Left out once it has reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<i32>,
}

/// One peer `daifuku restore` looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredPeer {
    /// Its label, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Its folder.
    pub directory: String,
    /// The session it resumes.
    pub session_id: String,
    /// The terminal it opened in; left out on a dry run, after `forget` and
    /// when it did not open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    /// Why it did not open, when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One terminal `daifuku stop-agents` closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoppedTerminal {
    /// Its window.
    pub window: u64,
    /// What to call it, as `daifuku status` does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The fleet it was in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet: Option<String>,
    /// Its cell in that fleet, counted from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<u32>,
    /// The session its agent last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The folder that session works in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Whether `daifuku restore` opens it again, resuming that session.
    pub kept: bool,
    /// Whether it closed; false on a dry run, and when Windows Terminal
    /// kept it open.
    pub closed: bool,
}

/// One process an agent started, which `daifuku stop-agents` ends when it
/// outlives the agent's terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leftover {
    /// Its id.
    pub pid: u32,
    /// Its executable's file name.
    pub name: String,
    /// The terminal whose agent started it.
    pub window: u64,
    /// Whether it was ended here; false on a dry run, and for one that
    /// ended by itself with its terminal.
    pub ended: bool,
    /// Why it still runs when it was not ended: it shows a window of its
    /// own, which may be one of yours now, or Windows refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One terminal `daifuku spawn` opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerStatus {
    /// Its window.
    pub window: u64,
    /// Its label, when it was given one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The folder it was opened in.
    pub directory: String,
    /// The last session its agent reported, which `daifuku restore`
    /// resumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// One open fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetStatus {
    /// Its name.
    pub name: String,
    /// The monitor it is on.
    pub monitor: String,
    /// Its terminals, in cell order.
    pub windows: Vec<u64>,
}

/// One window an agent reports from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWindow {
    /// The window.
    pub window: u64,
    /// Its title right now.
    pub title: String,
    /// What it shows.
    pub state: AgentState,
    /// How long it has shown that, in seconds.
    #[serde(default)]
    pub for_seconds: u64,
    /// What to call it: the label it was given, else its fleet's name and
    /// its number in the fleet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The fleet it is in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet: Option<String>,
    /// The session whose state it shows, as the agent names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The folder that session works in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The tool it ran last, or is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<String>,
    /// The text of its last notification, such as what it asks permission
    /// for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
    /// Its transcript file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// The account it runs on, by its name under `accounts`, once its
    /// status line has reported or its fleet names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Whether its account has reached its `warn`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub warning: bool,
}

/// Encodes one message as a line.
///
/// # Errors
///
/// Only if serialisation fails, which these types cannot.
pub fn to_line<T: Serialize>(message: &T) -> serde_json::Result<String> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

/// Decodes one line.
///
/// # Errors
///
/// When the line is not a message of that type.
pub fn from_line<'a, T: Deserialize<'a>>(line: &'a str) -> serde_json::Result<T> {
    serde_json::from_str(line.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_readable_json() {
        assert_eq!(to_line(&Request::Snap).unwrap(), "{\"command\":\"snap\"}\n");
        assert_eq!(
            to_line(&Request::Open {
                fleet: Some("mochi".into())
            })
            .unwrap(),
            "{\"command\":\"open\",\"fleet\":\"mochi\"}\n"
        );
        assert_eq!(
            to_line(&Request::Open { fleet: None }).unwrap(),
            "{\"command\":\"open\"}\n"
        );
    }

    #[test]
    fn every_request_round_trips() {
        for r in [
            Request::Open { fleet: None },
            Request::Open {
                fleet: Some("x".into()),
            },
            Request::Snap,
            Request::Close { fleet: None },
            Request::Close {
                fleet: Some("x".into()),
            },
            Request::CloseAll,
            Request::CloseTerminal {
                terminal: "site".into(),
            },
            Request::Next,
            Request::Demo,
            Request::Spawn(Spawn {
                directory: r"C:\src\site".into(),
                label: Some("site".into()),
                resume: Some("abc-1".into()),
                ..Spawn::default()
            }),
            Request::Spawn(Spawn {
                directory: r"C:\src\site".into(),
                fleet: Some("crew".into()),
                cell: Some(crate::peer::Cell::Free),
                ..Spawn::default()
            }),
            Request::Restore {
                dry_run: true,
                forget: false,
            },
            Request::Label {
                window: 5,
                label: Some("site".into()),
            },
            Request::Label {
                window: 5,
                label: None,
            },
            Request::StopAgents { dry_run: true },
            Request::StopAgents { dry_run: false },
            Request::Status,
            Request::Reload,
            Request::Stop,
        ] {
            let line = to_line(&r).unwrap();
            assert_eq!(from_line::<Request>(&line).unwrap(), r);
        }
    }

    #[test]
    fn every_response_round_trips() {
        let status = Status {
            version: "0.1.0".into(),
            config: "C:\\ProgramData\\Daifuku\\daifuku.json".into(),
            elevated: true,
            hotkeys: vec!["ctrl + alt + return: open agents".into()],
            fleets: vec![FleetStatus {
                name: "agents".into(),
                monitor: "portrait".into(),
                windows: vec![1, 2],
            }],
            agents: vec![AgentWindow {
                window: 1,
                title: "claude".into(),
                state: AgentState::Waiting,
                for_seconds: 73,
                label: Some("agents 1".into()),
                fleet: Some("agents".into()),
                session_id: Some("abc".into()),
                cwd: Some(r"C:\src\site".into()),
                last_tool: Some("Bash".into()),
                last_message: Some("Claude needs your permission to use Bash".into()),
                transcript_path: Some(r"C:\t\abc.jsonl".into()),
                account: Some("work".into()),
                warning: true,
            }],
            peers: vec![PeerStatus {
                window: 3,
                label: Some("site".into()),
                directory: r"C:\src\site".into(),
                session_id: None,
            }],
            usage: vec![AccountUsage {
                account: Some("work".into()),
                folder: r"C:\Users\me\.claude-work".into(),
                age_seconds: 12,
                five_hour: Some(WindowUsage {
                    used: 80,
                    resets_at: 1_792_548_000,
                    resets_in_seconds: Some(600),
                    pace: Some(-3),
                }),
                seven_day: None,
                warn: Some(77),
                warning: true,
            }],
        };
        for r in [
            Response::ok(),
            Response::said("opened 6"),
            Response::error("no fleet"),
            Response::Status(status),
            Response::Spawned {
                window: 7,
                label: Some("site".into()),
                fleet: Some("crew".into()),
                cell: Some(2),
            },
            Response::Spawned {
                window: 7,
                label: None,
                fleet: None,
                cell: None,
            },
            Response::Restored {
                peers: vec![RestoredPeer {
                    label: None,
                    directory: r"C:\src".into(),
                    session_id: "abc".into(),
                    window: Some(4),
                    error: None,
                }],
            },
            Response::Stopped {
                dry_run: false,
                terminals: vec![StoppedTerminal {
                    window: 4,
                    label: Some("crew 2".into()),
                    fleet: Some("crew".into()),
                    cell: Some(2),
                    session_id: Some("abc".into()),
                    directory: Some(r"C:\src".into()),
                    kept: true,
                    closed: true,
                }],
                leftovers: vec![Leftover {
                    pid: 4242,
                    name: "node.exe".into(),
                    window: 4,
                    ended: false,
                    reason: Some("access is denied".into()),
                }],
            },
        ] {
            let line = to_line(&r).unwrap();
            assert_eq!(from_line::<Response>(&line).unwrap(), r);
        }
    }

    #[test]
    fn a_spawn_is_one_flat_object() {
        let r = Request::Spawn(Spawn {
            directory: r"C:\src".into(),
            ..Spawn::default()
        });
        assert_eq!(
            to_line(&r).unwrap(),
            "{\"command\":\"spawn\",\"directory\":\"C:\\\\src\"}\n"
        );
        assert_eq!(
            to_line(&Response::Spawned {
                window: 9,
                label: None,
                fleet: None,
                cell: None,
            })
            .unwrap(),
            "{\"result\":\"spawned\",\"window\":9}\n"
        );
    }

    #[test]
    fn a_hook_message_carries_the_event_unchanged() {
        let m = HookMessage {
            window: 0x00b8_089e,
            event: HookEvent {
                session_id: "s".into(),
                hook_event_name: "PermissionRequest".into(),
                ..HookEvent::default()
            },
            pid: Some(4242),
        };
        assert_eq!(from_line::<HookMessage>(&to_line(&m).unwrap()).unwrap(), m);
        // A hook from before the field still reads.
        let old = r#"{"window":5,"event":{"session_id":"s","hook_event_name":"Stop"}}"#;
        assert_eq!(from_line::<HookMessage>(old).unwrap().pid, None);
    }

    #[test]
    fn stop_agents_reads_as_its_command_name() {
        assert_eq!(
            to_line(&Request::StopAgents { dry_run: true }).unwrap(),
            "{\"command\":\"stop-agents\",\"dry_run\":true}\n"
        );
        // A caller that leaves the flag out stops for real, as the command
        // line does without --dry-run.
        assert_eq!(
            from_line::<Request>("{\"command\":\"stop-agents\"}").unwrap(),
            Request::StopAgents { dry_run: false }
        );
    }

    #[test]
    fn the_hook_pipe_tells_an_event_from_a_usage_report() {
        let event = r#"{"window":5,"event":{"session_id":"s","hook_event_name":"Stop"}}"#;
        assert!(matches!(
            from_line::<HookLine>(event).unwrap(),
            HookLine::Event(_)
        ));
        let report = HookLine::Usage(UsageReport {
            usage: Limits {
                five_hour: Some(crate::usage::Window {
                    used: 12,
                    resets_at: 99,
                }),
                seven_day: None,
            },
            folder: r"C:\Users\me\.claude".into(),
            window: Some(5),
        });
        let line = to_line(&report).unwrap();
        assert_eq!(from_line::<HookLine>(&line).unwrap(), report);
        // An event with a window but no event, or a report with no usage,
        // is neither.
        assert!(from_line::<HookLine>(r#"{"window":5}"#).is_err());
        assert!(from_line::<HookLine>(r#"{"folder":"C:\\x"}"#).is_err());
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(from_line::<Request>("{\"command\":\"format-c\"}").is_err());
        assert!(from_line::<HookMessage>("").is_err());
    }
}
