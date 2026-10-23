//! What travels over Daifuku's two pipes, one JSON object per line.
//!
//! There are two pipes because there are two kinds of caller:
//!
//! - **The hook pipe** takes [`HookMessage`] from agent hooks, which run at
//!   whatever integrity the agent runs at. The worst a message can do is
//!   colour a border, so any process of the signed-in user may write to it.
//! - **The control pipe** takes [`Request`] from the command line. A request
//!   can open a fleet of administrator terminals when the daemon runs
//!   elevated, so then only an elevated process may connect; letting an
//!   ordinary process drive it would hand that process administrator rights.
//!   A daemon with the user's own rights lets in that user alone.

use serde::{Deserialize, Serialize};

use crate::config::Area;
use crate::peer::Spawn;
use crate::state::{AgentState, HookEvent};

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
        /// The monitor it was put on outside a grid, by device name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<String>,
        /// The part of that monitor it fills.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        area: Option<Area>,
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
            }],
            peers: vec![PeerStatus {
                window: 3,
                label: Some("site".into()),
                directory: r"C:\src\site".into(),
                session_id: None,
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
                monitor: None,
                area: None,
            },
            Response::Spawned {
                window: 7,
                label: None,
                fleet: None,
                cell: None,
                monitor: Some(r"\\.\DISPLAY1".into()),
                area: Some(Area::Right),
            },
            Response::Spawned {
                window: 7,
                label: None,
                fleet: None,
                cell: None,
                monitor: None,
                area: None,
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
                monitor: None,
                area: None,
            })
            .unwrap(),
            "{\"result\":\"spawned\",\"window\":9}\n"
        );
        assert_eq!(
            to_line(&Response::Spawned {
                window: 9,
                label: None,
                fleet: None,
                cell: None,
                monitor: Some(r"\\.\DISPLAY1".into()),
                area: Some(Area::Right),
            })
            .unwrap(),
            "{\"result\":\"spawned\",\"window\":9,\"monitor\":\"\\\\\\\\.\\\\DISPLAY1\",\"area\":\"right\"}\n"
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
    fn garbage_is_an_error_not_a_panic() {
        assert!(from_line::<Request>("{\"command\":\"format-c\"}").is_err());
        assert!(from_line::<HookMessage>("").is_err());
    }
}
