//! What travels over Daifuku's two pipes, one JSON object per line.
//!
//! There are two pipes because there are two kinds of caller:
//!
//! - **The hook pipe** takes [`HookMessage`] from agent hooks, which run at
//!   whatever integrity the agent runs at. The worst a message can do is
//!   colour a border, so any process of the signed-in user may write to it.
//! - **The control pipe** takes [`Request`] from the command line. A request
//!   can open a fleet of administrator terminals, so only an elevated process
//!   may connect. The daemon runs elevated; letting an ordinary process drive
//!   it would hand that process administrator rights.

use serde::{Deserialize, Serialize};

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
    /// Focus the terminal that has waited longest.
    Next,
    /// Open six scripted demo agents, to see Daifuku without a real one.
    Demo,
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
            Request::Next,
            Request::Demo,
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
            }],
        };
        for r in [
            Response::ok(),
            Response::said("opened 6"),
            Response::error("no fleet"),
            Response::Status(status),
        ] {
            let line = to_line(&r).unwrap();
            assert_eq!(from_line::<Response>(&line).unwrap(), r);
        }
    }

    #[test]
    fn a_hook_message_carries_the_event_unchanged() {
        let m = HookMessage {
            window: 0x00b8_089e,
            event: HookEvent {
                session_id: "s".into(),
                hook_event_name: "PermissionRequest".into(),
                notification_type: None,
            },
        };
        assert_eq!(from_line::<HookMessage>(&to_line(&m).unwrap()).unwrap(), m);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(from_line::<Request>("{\"command\":\"format-c\"}").is_err());
        assert!(from_line::<HookMessage>("").is_err());
    }
}
