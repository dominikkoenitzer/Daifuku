//! Peers: agent terminals opened one at a time with `daifuku spawn`, beside
//! the fleets, each in the folder it works in.
//!
//! A peer starts from a fleet's definition, the first one in the config
//! unless another is named, and takes its command, profile and administrator
//! setting. It runs in an environment of its own: the one Windows builds for
//! the user, without any variable an agent set for itself. A peer started
//! from inside another agent would otherwise take that agent's session for
//! its own.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{Area, MonitorPick};

/// The longest label a peer takes, in characters.
pub const MAX_LABEL: usize = 200;

/// The longest session id a peer resumes, in characters. Claude Code's are
/// 36, a UUID.
pub const MAX_SESSION_ID: usize = 128;

/// What `daifuku spawn` asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawn {
    /// The folder the agent works in.
    pub directory: PathBuf,
    /// The terminal's title, for people and for `daifuku status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A session to resume, by its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// The fleet whose command, profile and administrator setting the peer
    /// takes. The first fleet in the config when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet: Option<String>,
    /// The monitor to put it on. With neither this nor `area`, the peer
    /// stays where Windows Terminal puts it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<MonitorPick>,
    /// The part of the monitor it fills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<Area>,
}

impl Spawn {
    /// What is wrong with the request, if anything: a label with control
    /// characters or past [`MAX_LABEL`], a session id that is not one.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        if let Some(label) = &self.label
            && (label.chars().count() > MAX_LABEL || label.chars().any(char::is_control))
        {
            return Some(format!(
                "a label is at most {MAX_LABEL} characters, without line breaks or tabs"
            ));
        }
        if let Some(id) = &self.resume
            && !session_id_ok(id)
        {
            return Some(format!("`{id}` is not a session id"));
        }
        None
    }

    /// Whether the peer is to be placed, rather than left where Windows
    /// Terminal opens it.
    #[must_use]
    pub const fn placed(&self) -> bool {
        self.monitor.is_some() || self.area.is_some()
    }
}

/// Whether `id` can be a session id: a letter or digit, then letters,
/// digits, `-` and `_`, up to [`MAX_SESSION_ID`]. It goes into a command line
/// an administrator terminal runs, so nothing else may, and it cannot pass
/// for an option.
#[must_use]
pub fn session_id_ok(id: &str) -> bool {
    id.len() <= MAX_SESSION_ID
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `command` resuming session `id`, as Claude Code takes it.
#[must_use]
pub fn resume_command(command: &str, id: &str) -> String {
    format!("{command} --resume {id}")
}

/// Whether a variable is one an agent sets for itself and its children:
/// every name that starts with `CLAUDE`, in any case, `CLAUDECODE` included.
#[must_use]
pub fn is_agent_variable(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n.len() >= 6 && n.as_bytes()[..6].eq_ignore_ascii_case(b"CLAUDE"))
}

/// `vars` without the agents' own variables.
pub fn without_agent_variables(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    vars.into_iter()
        .filter(|(name, _)| !is_agent_variable(name))
        .collect()
}

