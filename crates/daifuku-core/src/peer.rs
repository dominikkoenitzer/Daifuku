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

use crate::config::{Area, MonitorPick, Priority};

/// The longest label a peer takes, in characters.
pub const MAX_LABEL: usize = 200;

/// The longest session id a peer resumes, in characters. Claude Code's are
/// 36, a UUID.
pub const MAX_SESSION_ID: usize = 128;

/// Which cell of its fleet's grid a peer takes, written `"free"` or as the
/// cell's number from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// The first one that is free.
    Free,
    /// This one, or the first one free when it is taken or past the grid,
    /// as after the fleet's count was lowered.
    At(u32),
}

impl Cell {
    /// The cell this one names, counted from 1, if any.
    #[must_use]
    pub const fn number(self) -> Option<u32> {
        match self {
            Self::Free => None,
            Self::At(n) => Some(n),
        }
    }
}

impl std::str::FromStr for Cell {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("free") {
            return Ok(Self::Free);
        }
        match s.parse::<u32>() {
            Ok(n) if n >= 1 => Ok(Self::At(n)),
            _ => Err(format!("`{s}` is not a cell: free, or a number from 1")),
        }
    }
}

impl std::fmt::Display for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Free => f.write_str("free"),
            Self::At(n) => write!(f, "{n}"),
        }
    }
}

impl Serialize for Cell {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Free => s.serialize_str("free"),
            Self::At(n) => s.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Number(u32),
            Word(String),
        }
        match Written::deserialize(d)? {
            Written::Number(n) if n >= 1 => Ok(Self::At(n)),
            Written::Number(n) => n.to_string().parse().map_err(serde::de::Error::custom),
            Written::Word(w) => w.parse().map_err(serde::de::Error::custom),
        }
    }
}

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
    /// The priority its group runs at, instead of its fleet's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// A cell of its fleet's grid to take, so it becomes one of the fleet's
    /// terminals: placed, snapped and closed with them. Kept as the cell it
    /// took, so `daifuku restore` puts it back there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<Cell>,
}

impl Spawn {
    /// What is wrong with the request, if anything: a label with control
    /// characters or past [`MAX_LABEL`], a session id that is not one.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        if let Some(label) = &self.label
            && (label.chars().count() > MAX_LABEL
                || label.chars().any(char::is_control)
                || label.starts_with('-'))
        {
            return Some(format!(
                "a label is at most {MAX_LABEL} characters, without line breaks or tabs, and does not start with -"
            ));
        }
        if let Some(id) = &self.resume
            && !session_id_ok(id)
        {
            return Some(format!("`{id}` is not a session id"));
        }
        if self.cell.is_some() && self.placed() {
            return Some(
                "a peer in a cell goes where its fleet's grid puts it: leave out the monitor and the area"
                    .to_owned(),
            );
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

/// `command` resuming session `id`: right after the agent's program, in the
/// statement that starts it, so the statements before it, such as one that
/// sets a variable or changes folder, stay as they are. The agent is
/// `claude` or `codex` by any path, or a package of theirs a package runner
/// such as `npx` starts. Codex takes `codex resume <id>`, Claude Code
/// `--resume <id>`. A command with no agent in it gets `--resume <id>` at
/// its end.
#[must_use]
pub fn resume_command(command: &str, id: &str) -> String {
    match agent_in(command) {
        Some((end, Agent::Codex)) => {
            format!("{} resume {id}{}", &command[..end], &command[end..])
        }
        Some((end, Agent::Claude)) => {
            format!("{} --resume {id}{}", &command[..end], &command[end..])
        }
        None => format!("{command} --resume {id}"),
    }
}

/// The agents a command can resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agent {
    Claude,
    Codex,
}

/// Where the agent's program ends in `line`, and which agent it is: the
/// first statement whose program is an agent, or a package runner that
/// starts one.
fn agent_in(line: &str) -> Option<(usize, Agent)> {
    for statement in statements(line) {
        let mut words = statement.iter();
        let Some((first, end)) = words.next() else {
            continue;
        };
        if let Some(agent) = agent_named(first) {
            return Some((*end, agent));
        }
        if is_package_runner(first) {
            // `npx -y pkg`, `pnpm dlx pkg`, `bun x pkg`: the package is the
            // first word that is no option and no subcommand.
            if let Some((package, end)) = words
                .find(|(w, _)| !w.starts_with('-') && !matches!(w.as_str(), "x" | "dlx" | "exec"))
                && let Some(agent) = agent_named(package)
            {
                return Some((*end, agent));
            }
        }
    }
    None
}

/// The agent a program or package names: its last path part, without a
/// version after `@` or an extension. A word that sets a variable is none,
/// whatever its value says.
fn agent_named(word: &str) -> Option<Agent> {
    if word.starts_with('$') || word.contains('=') {
        return None;
    }
    let last = word.rsplit(['\\', '/']).next().unwrap_or(word);
    let name = last.split('@').next().unwrap_or_default();
    match name.split('.').next()?.to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        _ => None,
    }
}

/// Whether `word` is a program that runs a package by its name.
fn is_package_runner(word: &str) -> bool {
    let last = word.rsplit(['\\', '/']).next().unwrap_or(word);
    let stem = last
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        stem.as_str(),
        "npx" | "pnpx" | "bunx" | "npm" | "pnpm" | "yarn" | "bun"
    )
}

/// The statements of a PowerShell line, each as its words with where each
/// word ends in the line. Quotes hold a word together and are not part of
/// it; `;`, `|`, `&&` and `||` outside them end a statement, and `&` alone
/// is the call operator, no word.
fn statements(line: &str) -> Vec<Vec<(String, usize)>> {
    let mut all: Vec<Vec<(String, usize)>> = vec![Vec::new()];
    let mut word: Option<(String, usize)> = None;
    let mut quote: Option<char> = None;
    let mut chars = line.char_indices().peekable();
    let finish = |all: &mut Vec<Vec<(String, usize)>>, word: &mut Option<(String, usize)>| {
        if let Some(w) = word.take()
            && let Some(statement) = all.last_mut()
        {
            statement.push(w);
        }
    };
    while let Some((i, c)) = chars.next() {
        let after = i + c.len_utf8();
        if let Some(q) = quote {
            let (text, end) = word.get_or_insert_with(|| (String::new(), after));
            if c == q {
                quote = None;
            } else {
                text.push(c);
            }
            *end = after;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                word.get_or_insert_with(|| (String::new(), after)).1 = after;
            }
            ';' | '|' => {
                finish(&mut all, &mut word);
                all.push(Vec::new());
            }
            '&' => {
                finish(&mut all, &mut word);
                if chars.next_if(|&(_, n)| n == '&').is_some() {
                    all.push(Vec::new());
                }
            }
            c if c.is_whitespace() => finish(&mut all, &mut word),
            c => {
                let (text, end) = word.get_or_insert_with(|| (String::new(), after));
                text.push(c);
                *end = after;
            }
        }
    }
    finish(&mut all, &mut word);
    all
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

/// How far the start of Windows may drift between a saved peer and its use,
/// in seconds, as for the daemon's saved state.
const BOOT_DRIFT_SECONDS: u64 = 10;

/// One peer as the daemon keeps it in `peers.json`, so it can be opened
/// again after Windows restarts: what it was opened with, with `resume` set
/// to the session it ran last, and the window and the start of Windows it
/// was open in, so a daemon that restarts while it is still open finds it
/// again instead of opening it twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// What it was opened with, `resume` the last session seen in it.
    pub spawn: Spawn,
    /// Its window, as long as it is open.
    pub window: u64,
    /// When Windows started, in seconds since 1970 by the wall clock, for
    /// the start of Windows the window belongs to.
    pub boot: u64,
    /// Closed with its fleet, by `daifuku close` or the close key, in that
    /// start of Windows: `daifuku restore` opens it again as it would one
    /// that was open when Windows stopped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed_with_fleet: bool,
    /// The fleet and the cell it was in while it was open, so a daemon that
    /// finds it open puts it back there. The daemon's saved fleets go stale
    /// after a few minutes; the peer would then be outside its fleet, and
    /// opening the fleet would start a terminal over it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_cell: Option<FleetCell>,
}

/// A cell of an open fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetCell {
    /// The config's name for the fleet.
    pub fleet: String,
    /// The cell, counted from 1.
    pub cell: u32,
}

impl Kept {
    /// Whether its window can still be the one it was open in: the same
    /// start of Windows, by the clock `boot` the caller works out now.
    #[must_use]
    pub const fn same_boot(&self, boot: u64) -> bool {
        self.boot.abs_diff(boot) <= BOOT_DRIFT_SECONDS
    }

    /// Whether it can be opened again: only with a session to resume, as a
    /// new session in its folder is not the one that was there.
    #[must_use]
    pub fn resumable(&self) -> bool {
        self.spawn.resume.as_deref().is_some_and(session_id_ok)
    }
}

/// What the peers file holds: the start of Windows it was written in, the
/// one its dormant peers are from, and the peers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptFile {
    /// When Windows started, for the start of Windows the file was written
    /// in.
    pub boot: u64,
    /// The start of Windows the dormant peers in it were open in.
    #[serde(default)]
    pub dormant_boot: u64,
    /// Every peer, open and dormant.
    pub peers: Vec<Kept>,
}

/// The peers file. One that does not read is an error, not an empty file,
/// so it is never saved over; an empty file holds no peers. An entry whose
/// session id is not one keeps no session.
///
/// # Errors
///
/// When the text is not a peers file this version reads.
pub fn read_kept(text: &str) -> Result<KeptFile, serde_json::Error> {
    if text.trim().is_empty() {
        return Ok(KeptFile::default());
    }
    let mut file: KeptFile = serde_json::from_str(text)?;
    for k in &mut file.peers {
        if k.spawn
            .resume
            .as_deref()
            .is_some_and(|id| !session_id_ok(id))
        {
            k.spawn.resume = None;
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_claude_variable_goes_and_nothing_else() {
        let vars = [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_CHILD_SESSION",
            "Claude_Config_Dir",
            "claude_x",
            "PATH",
            "USERPROFILE",
            "MY_CLAUDE",
            "CLAUD",
            "ANTHROPIC_API_KEY",
        ]
        .map(|n| (OsString::from(n), OsString::from("1")));
        let kept: Vec<_> = without_agent_variables(vars)
            .into_iter()
            .map(|(n, _)| n.into_string().unwrap())
            .collect();
        assert_eq!(
            kept,
            [
                "PATH",
                "USERPROFILE",
                "MY_CLAUDE",
                "CLAUD",
                "ANTHROPIC_API_KEY"
            ]
        );
    }

    #[test]
    fn only_plain_ids_are_session_ids() {
        for id in ["0e9b6c1a-3f42-4a8e-9a77-2b1c5d6e7f80", "abc", "a_b-C9"] {
            assert!(session_id_ok(id), "{id}");
        }
        let long = "a".repeat(MAX_SESSION_ID + 1);
        for id in [
            "", "a b", "a;b", "a'b", "$(x)", "a\nb", "ä", "--help", "_a", &long,
        ] {
            assert!(!session_id_ok(id), "{id}");
        }
    }

    fn kept(resume: Option<&str>, window: u64, boot: u64) -> Kept {
        Kept {
            spawn: Spawn {
                directory: r"C:\src".into(),
                resume: resume.map(Into::into),
                ..Spawn::default()
            },
            window,
            boot,
            closed_with_fleet: false,
            in_cell: None,
        }
    }

    #[test]
    fn a_kept_peer_knows_its_start_of_windows_and_whether_it_resumes() {
        let k = kept(Some("abc"), 7, 1_000);
        assert!(k.same_boot(1_000) && k.same_boot(1_010) && k.same_boot(990));
        assert!(!k.same_boot(1_011) && !k.same_boot(5_000));
        assert!(k.resumable());
        assert!(!kept(None, 7, 1_000).resumable());
    }

    #[test]
    fn a_kept_peer_keeps_the_cell_of_the_fleet_it_was_in() {
        let k = Kept {
            in_cell: Some(FleetCell {
                fleet: "crew".into(),
                cell: 2,
            }),
            ..kept(Some("abc"), 7, 1_000)
        };
        let text = serde_json::to_string(&k).unwrap();
        assert!(
            text.contains(r#""in_cell":{"fleet":"crew","cell":2}"#),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<Kept>(&text).unwrap(), k);
        // One outside a fleet writes none, and peers kept before read as
        // outside one.
        let plain = serde_json::to_string(&kept(Some("abc"), 7, 1_000)).unwrap();
        assert!(!plain.contains("in_cell"), "{plain}");
        assert_eq!(serde_json::from_str::<Kept>(&plain).unwrap().in_cell, None);
    }

    #[test]
    fn the_kept_file_drops_ids_that_are_not_ids_and_refuses_what_does_not_read() {
        let file = KeptFile {
            boot: 1_000,
            dormant_boot: 500,
            peers: vec![kept(Some("abc-1"), 7, 1_000), kept(Some("a;rm"), 8, 1_000)],
        };
        let read = read_kept(&serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!((read.boot, read.dormant_boot), (1_000, 500));
        assert_eq!(read.peers[0], kept(Some("abc-1"), 7, 1_000));
        assert_eq!(read.peers[1].spawn.resume, None);
        assert!(read_kept("").unwrap().peers.is_empty());
        assert!(read_kept(" \n").unwrap().peers.is_empty());
        // Saving over any of these would lose the peers in them.
        assert!(read_kept("not json").is_err());
        assert!(read_kept(r#"{"boot": 1, "peers": ["#).is_err());
        // The list alone, as no released version wrote.
        assert!(read_kept("[]").is_err());
    }

    #[test]
    fn resuming_puts_the_id_right_after_the_agent() {
        for (command, resumed) in [
            (
                "claude --dangerously-skip-permissions",
                "claude --resume abc-1 --dangerously-skip-permissions",
            ),
            ("claude", "claude --resume abc-1"),
            // A statement before the agent stays as it is, even one whose
            // value looks like the agent.
            (
                r"$env:CLAUDE_CONFIG_DIR='C:\claude'; claude --dangerously-skip-permissions",
                r"$env:CLAUDE_CONFIG_DIR='C:\claude'; claude --resume abc-1 --dangerously-skip-permissions",
            ),
            (r"cd C:\src; claude", r"cd C:\src; claude --resume abc-1"),
            (
                "Write-Host hi && codex --yolo",
                "Write-Host hi && codex resume abc-1 --yolo",
            ),
            // Started by a package runner: after the package.
            (
                "npx @anthropic-ai/claude-code",
                "npx @anthropic-ai/claude-code --resume abc-1",
            ),
            (
                "npx -y @anthropic-ai/claude-code@latest --model opus",
                "npx -y @anthropic-ai/claude-code@latest --resume abc-1 --model opus",
            ),
            (
                "pnpm dlx @openai/codex --yolo",
                "pnpm dlx @openai/codex resume abc-1 --yolo",
            ),
            (
                r#"& "C:\Program Files\x\claude.exe" --model opus"#,
                r#"& "C:\Program Files\x\claude.exe" --resume abc-1 --model opus"#,
            ),
            // Codex resumes with a subcommand, its flags after it.
            (
                "codex --dangerously-bypass-approvals-and-sandbox",
                "codex resume abc-1 --dangerously-bypass-approvals-and-sandbox",
            ),
            (
                r"& 'C:\tools\Codex.CMD' -m o4",
                r"& 'C:\tools\Codex.CMD' resume abc-1 -m o4",
            ),
            ("  codex.exe|Out-Host", "  codex.exe resume abc-1|Out-Host"),
            // No agent to be found: at the end, as before.
            ("my-agent --flag", "my-agent --flag --resume abc-1"),
            ("npx some-tool", "npx some-tool --resume abc-1"),
            // A word after a program that runs no package is no agent.
            (
                "Write-Host claude; codex",
                "Write-Host claude; codex resume abc-1",
            ),
            // Nor is a variable set the way other shells write it.
            (
                r"CLAUDE_CONFIG_DIR=C:\claude claude",
                r"CLAUDE_CONFIG_DIR=C:\claude claude --resume abc-1",
            ),
        ] {
            assert_eq!(resume_command(command, "abc-1"), resumed, "{command}");
        }
    }

    #[test]
    fn a_line_splits_into_statements_of_words_outside_quotes() {
        let words = |line: &str| -> Vec<Vec<String>> {
            statements(line)
                .into_iter()
                .map(|s| s.into_iter().map(|(w, _)| w).collect())
                .collect()
        };
        assert_eq!(
            words(r#"& "C:\a b\c.exe" -x 'y; z'; d|e && f"#),
            [
                vec![r"C:\a b\c.exe", "-x", "y; z"],
                vec!["d"],
                vec!["e"],
                vec!["f"],
            ]
        );
        let line = r#"a "b c" d"#;
        let ends: Vec<usize> = statements(line)[0].iter().map(|&(_, e)| e).collect();
        assert_eq!(ends, [1, 7, 9]);
    }

    #[test]
    fn a_bad_label_or_session_id_is_named() {
        let ok = Spawn {
            directory: r"C:\src\site".into(),
            label: Some("Site QA".into()),
            resume: Some("abc-1".into()),
            ..Spawn::default()
        };
        assert_eq!(ok.problem(), None);
        let tab = Spawn {
            label: Some("a\tb".into()),
            ..ok.clone()
        };
        assert!(tab.problem().unwrap().contains("label"));
        // Windows Terminal would read it as an option.
        let dash = Spawn {
            label: Some("--help".into()),
            ..ok.clone()
        };
        assert!(dash.problem().unwrap().contains("label"));
        let long = Spawn {
            label: Some("x".repeat(MAX_LABEL + 1)),
            ..ok.clone()
        };
        assert!(long.problem().is_some());
        let longest = Spawn {
            label: Some("x".repeat(MAX_LABEL)),
            ..ok.clone()
        };
        assert_eq!(longest.problem(), None);
        let id = Spawn {
            resume: Some("a;b".into()),
            ..ok
        };
        assert_eq!(id.problem().unwrap(), "`a;b` is not a session id");
    }

    #[test]
    fn a_cell_is_free_or_a_number_from_one() {
        assert_eq!("free".parse::<Cell>(), Ok(Cell::Free));
        assert_eq!("Free".parse::<Cell>(), Ok(Cell::Free));
        assert_eq!("3".parse::<Cell>(), Ok(Cell::At(3)));
        for bad in ["0", "-1", "", "first", "1.5"] {
            assert!(bad.parse::<Cell>().is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_string(&Cell::Free).unwrap(), r#""free""#);
        assert_eq!(serde_json::to_string(&Cell::At(2)).unwrap(), "2");
        assert_eq!(serde_json::from_str::<Cell>("2").unwrap(), Cell::At(2));
        assert_eq!(
            serde_json::from_str::<Cell>(r#""free""#).unwrap(),
            Cell::Free
        );
        assert!(serde_json::from_str::<Cell>("0").is_err());
        assert!(serde_json::from_str::<Cell>(r#""2""#).is_ok());
        assert_eq!((Cell::At(2).number(), Cell::Free.number()), (Some(2), None));
        assert_eq!(
            (Cell::At(2).to_string(), Cell::Free.to_string()),
            ("2".into(), "free".into())
        );
    }

    #[test]
    fn a_peer_in_a_cell_takes_no_monitor_or_area_of_its_own() {
        let in_cell = Spawn {
            directory: r"C:\src".into(),
            cell: Some(Cell::Free),
            ..Spawn::default()
        };
        assert_eq!(in_cell.problem(), None);
        for placed in [
            Spawn {
                area: Some(Area::Right),
                ..in_cell.clone()
            },
            Spawn {
                monitor: Some(MonitorPick::Portrait),
                ..in_cell.clone()
            },
        ] {
            assert!(placed.problem().unwrap().contains("cell"), "{placed:?}");
        }
        // Peers kept before cells existed still read.
        let old: Spawn = serde_json::from_str(r#"{"directory":"C:\\src"}"#).unwrap();
        assert_eq!(old.cell, None);
        let text = serde_json::to_string(&Spawn {
            cell: Some(Cell::At(2)),
            ..in_cell
        })
        .unwrap();
        assert_eq!(text, r#"{"directory":"C:\\src","cell":2}"#);
    }

    #[test]
    fn a_spawn_round_trips_and_leaves_out_what_is_not_set() {
        let s = Spawn {
            directory: r"C:\src".into(),
            area: Some(Area::Right),
            monitor: Some(MonitorPick::Landscape),
            ..Spawn::default()
        };
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(
            text,
            r#"{"directory":"C:\\src","monitor":"landscape","area":"right"}"#
        );
        assert_eq!(serde_json::from_str::<Spawn>(&text).unwrap(), s);
        assert!(s.placed());
        assert!(!Spawn::default().placed());
    }
}
