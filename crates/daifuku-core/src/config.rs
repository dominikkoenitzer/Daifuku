//! The config file, `C:\ProgramData\Daifuku\daifuku.json`.
//!
//! Every key is optional. An empty file, or no file, is a working setup: one
//! fleet of six terminals running `claude` with the user's own rights, on the
//! monitor turned on its side, or on the primary one when none is.
//!
//! ```json
//! {
//!   "$schema": "https://get-daifuku.vercel.app/schema.json",
//!   "fleets": [
//!     { "name": "agents", "count": 6, "monitor": "portrait", "hotkey": "ctrl + alt + return" },
//!     { "name": "site", "count": 4, "directory": "C:\\src\\site", "hotkey": "ctrl + alt + w" }
//!   ]
//! }
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::geometry::Rect;
use crate::grid::{Gaps, Shape};
use crate::hotkey::Hotkey;
use crate::state::AgentState;

/// Daifuku's own address. Every link Daifuku writes or prints starts here.
pub const SITE: &str = "https://get-daifuku.vercel.app";

/// The most terminals one fleet may open. Past this a cell is too small to
/// read, on any monitor sold today.
pub const MAX_FLEET: u32 = 16;

/// The whole config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Where the schema lives, for editors. Ignored by Daifuku.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Spacing around and between terminals.
    pub gaps: Gaps,
    /// The status border drawn around every terminal an agent reports from.
    pub border: Border,
    /// Keys that act on every fleet at once.
    pub hotkeys: Hotkeys,
    /// The fleets, each with its own hotkey.
    pub fleets: Vec<Fleet>,
    /// Accounts the agents sign in with, by a name of your own, for fleets
    /// that should not all run on the one in your profile.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub accounts: BTreeMap<String, Account>,
    /// Play Windows' Asterisk sound when an agent starts waiting for you,
    /// for when you are not looking at the screen.
    pub sound: bool,
    /// Start the daemon with administrator rights at sign-in, which fleets
    /// with `"admin": true` need. Off, it runs with your own rights. Takes
    /// effect at the next `daifuku install`, which registers the logon task.
    pub elevated: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: None,
            gaps: Gaps::default(),
            border: Border::default(),
            hotkeys: Hotkeys::default(),
            fleets: vec![Fleet::default()],
            accounts: BTreeMap::new(),
            sound: false,
            elevated: false,
        }
    }
}

/// An account, as the folders the agents keep it in: the sign-in, settings,
/// hooks and sessions. A fleet that names it starts its agents with these
/// folders, so one fleet can work on one subscription and the next on
/// another.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Account {
    /// Claude Code's folder, which it takes from `CLAUDE_CONFIG_DIR`. Left
    /// out, Claude Code uses its own, `.claude` in your profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<PathBuf>,
    /// Codex's folder, which it takes from `CODEX_HOME`. Left out, Codex
    /// uses its own, `.codex` in your profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<PathBuf>,
    /// Warn once its agents' status lines report this share of the 5-hour
    /// or the weekly limit used, in percent, 1 to 100: the borders of its
    /// terminals turn the warning colour until the window resets. Needs
    /// `daifuku status-line` in Claude Code's status line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn: Option<u8>,
    /// When its fleets get no more tasks from their backlog: the first rule
    /// that holds for a window, in the order written, says how much of it
    /// may be used. A window no rule holds for has no stop. Needs
    /// `daifuku status-line` in Claude Code's status line; with rules but no
    /// numbers reported yet, no task is handed out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_at: Vec<StopAt>,
}

/// One rule for when an account's fleets get no more tasks, such as
/// `{ "share": 75, "days": "mon-fri", "hours": "9-17" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopAt {
    /// No task is handed out once this share of the window is used, in
    /// percent, 1 to 100.
    pub share: u8,
    /// The window it is for, `five_hour` or `seven_day`. Left out, both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<LimitWindow>,
    /// The days it holds on, in local time: `mon-fri`, `sat,sun` or one
    /// day. Left out, every day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<String>,
    /// The hours it holds in, in local time, from the first up to the
    /// second, 0 to 24: `8-18`, or `22-6` past midnight. Left out, all day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hours: Option<String>,
    /// It holds only in this many minutes before the window resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_minutes: Option<u32>,
}

/// One of the two limits Claude Code reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LimitWindow {
    /// The 5-hour window.
    FiveHour,
    /// The weekly one.
    SevenDay,
}

/// The time a [`StopAt`] is checked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    /// Seconds since 1970, as windows reset at.
    pub now: u64,
    /// The local day, 0 for Monday to 6 for Sunday.
    pub weekday: u8,
    /// Local minutes since midnight.
    pub minute: u16,
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

impl StopAt {
    /// Whether it holds for `window`, which resets at `resets_at`, at
    /// `time`.
    #[must_use]
    pub fn holds(&self, window: LimitWindow, resets_at: u64, time: LocalTime) -> bool {
        self.window.is_none_or(|w| w == window)
            && self
                .days
                .as_deref()
                .is_none_or(|d| day_mask(d).is_ok_and(|m| m & (1 << time.weekday) != 0))
            && self.hours.as_deref().is_none_or(|h| {
                hour_span(h).is_ok_and(|(from, to)| {
                    let hour = time.minute / 60;
                    if from < to {
                        (from..to).contains(&hour)
                    } else {
                        hour >= from || hour < to
                    }
                })
            })
            && self
                .last_minutes
                .is_none_or(|m| resets_at > time.now && resets_at - time.now <= u64::from(m) * 60)
    }

    fn problem(&self) -> Option<String> {
        if !(1..=100).contains(&self.share) {
            return Some("a stop_at share is a percentage from 1 to 100".to_owned());
        }
        if let Some(Err(e)) = self.days.as_deref().map(day_mask) {
            return Some(e);
        }
        if let Some(Err(e)) = self.hours.as_deref().map(hour_span) {
            return Some(e);
        }
        if self
            .last_minutes
            .is_some_and(|m| !(1..=10_080).contains(&m))
        {
            return Some("last_minutes is 1 to 10080, a week".to_owned());
        }
        None
    }
}

/// `mon-fri`, `sat,sun` or `wed` as one bit per day, Monday the lowest.
fn day_mask(text: &str) -> Result<u8, String> {
    let day = |name: &str| {
        let name = name.trim().to_ascii_lowercase();
        DAYS.iter()
            .position(|d| *d == name)
            .ok_or_else(|| format!("days: `{name}` is not mon, tue, wed, thu, fri, sat or sun"))
    };
    let mut mask = 0u8;
    for part in text.split(',') {
        if let Some((from, to)) = part.split_once('-') {
            let (from, to) = (day(from)?, day(to)?);
            let mut d = from;
            loop {
                mask |= 1 << d;
                if d == to {
                    break;
                }
                d = (d + 1) % 7;
            }
        } else {
            mask |= 1 << day(part)?;
        }
    }
    Ok(mask)
}

/// `8-18` as its two hours, the first taken as 0 to 23.
fn hour_span(text: &str) -> Result<(u16, u16), String> {
    let bad = || format!("hours: `{text}` is not two hours 0 to 24, such as 8-18");
    let (from, to) = text.split_once('-').ok_or_else(bad)?;
    let hour = |h: &str| h.trim().parse::<u16>().ok().filter(|h| *h <= 24);
    let (from, to) = (hour(from).ok_or_else(bad)?, hour(to).ok_or_else(bad)?);
    if from % 24 == to % 24 {
        return Err(format!(
            "hours: `{text}` is no time at all; leave hours out for all day"
        ));
    }
    Ok((from % 24, to))
}

impl Account {
    /// The variables a terminal of this account starts with, each with the
    /// folder it names.
    #[must_use]
    pub fn variables(&self) -> Vec<(&'static str, &Path)> {
        [
            ("CLAUDE_CONFIG_DIR", self.claude.as_deref()),
            ("CODEX_HOME", self.codex.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, folder)| Some((name, folder?)))
        .collect()
    }
}

/// The status border.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Border {
    /// Draw status borders at all.
    pub enabled: bool,
    /// Thickness in physical pixels, 1 to 64; 0, which earlier versions took,
    /// draws as 1. `enabled: false` draws none.
    pub width: i32,
    /// How far outside the visible frame the border sits, -64 to 64 pixels;
    /// negative overlaps the window's own edge.
    pub offset: i32,
    /// The colours, as a set. `catppuccin` by default; `colorblind` is the
    /// Okabe-Ito palette, which stays distinct for every common kind of
    /// colour blindness.
    pub palette: Palette,
    /// Colours of your own, for any of the states. A state left out keeps
    /// the palette's colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colours: Option<OwnColours>,
    /// Draw each state at its own width, so a state reads without its
    /// colour: at a width of 4, done is 2, working 4, failed 6 and waiting 8.
    /// Always on with a high contrast theme.
    pub state_widths: bool,
    /// Let a waiting border breathe slowly, between 70 % and full brightness,
    /// the one thing on screen that moves. Off whenever Windows is set to
    /// show no animations or a high contrast theme is on.
    pub pulse: bool,
}

impl Default for Border {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 4,
            offset: 0,
            palette: Palette::default(),
            colours: None,
            state_widths: true,
            pulse: true,
        }
    }
}

impl Border {
    /// The colours in use: your own where you set one, else the palette's.
    #[must_use]
    pub fn colours(&self) -> StateColours {
        let palette = self.palette.colours();
        let own = self.colours.unwrap_or_default();
        StateColours {
            working: own.working.unwrap_or(palette.working),
            waiting: own.waiting.unwrap_or(palette.waiting),
            done: own.done.unwrap_or(palette.done),
            failed: own.failed.unwrap_or(palette.failed),
            warning: own.warning.unwrap_or(palette.warning),
        }
    }

    /// The thickness a state is drawn at. With state widths each state is
    /// thicker than the one before it, done, working, failed, waiting, even
    /// at a width of 1, where halving alone would give three of them 1.
    #[must_use]
    pub fn width_for(&self, state: AgentState) -> i32 {
        let w = self.width.max(1);
        if !self.state_widths {
            return w;
        }
        let done = (w / 2).max(1);
        let working = w.max(done + 1);
        let failed = (w + w / 2).max(working + 1);
        let waiting = (w * 2).max(failed + 1);
        match state {
            AgentState::Done => done,
            AgentState::Working => working,
            AgentState::Failed => failed,
            AgentState::Waiting => waiting,
        }
    }
}

/// A named set of state colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    /// Catppuccin Mocha: blue, yellow, green and red, and peach for an
    /// account's warning.
    #[default]
    Catppuccin,
    /// Okabe-Ito: sky blue, orange, bluish green and vermilion, and reddish
    /// purple for an account's warning, chosen to stay distinct for
    /// deuteranopia, protanopia and tritanopia.
    Colorblind,
}

impl Palette {
    /// The palette's colours.
    #[must_use]
    pub const fn colours(self) -> StateColours {
        match self {
            Self::Catppuccin => StateColours {
                working: Colour::new(0x89, 0xb4, 0xfa),
                waiting: Colour::new(0xf9, 0xe2, 0xaf),
                done: Colour::new(0xa6, 0xe3, 0xa1),
                failed: Colour::new(0xf3, 0x8b, 0xa8),
                warning: Colour::new(0xfa, 0xb3, 0x87),
            },
            Self::Colorblind => StateColours {
                working: Colour::new(0x56, 0xb4, 0xe9),
                waiting: Colour::new(0xe6, 0x9f, 0x00),
                done: Colour::new(0x00, 0x9e, 0x73),
                failed: Colour::new(0xd5, 0x5e, 0x00),
                warning: Colour::new(0xcc, 0x79, 0xa7),
            },
        }
    }
}

/// Colours of your own, as the file writes them. Each one is optional, so
/// overriding one state keeps the palette's colour for the other three.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct OwnColours {
    /// Thinking and calling tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working: Option<Colour>,
    /// Blocked on you, the one to look for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<Colour>,
    /// Finished its turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<Colour>,
    /// The turn ended on an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<Colour>,
    /// Working or done on an account that has reached its `warn`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<Colour>,
}

/// A colour for each agent state, as drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateColours {
    /// Thinking and calling tools. Blue.
    pub working: Colour,
    /// Blocked on you. Yellow, the one to look for.
    pub waiting: Colour,
    /// Finished its turn. Green.
    pub done: Colour,
    /// The turn ended on an error. Red.
    pub failed: Colour,
    /// Working or done on an account that has reached its `warn`. Peach.
    pub warning: Colour,
}

impl StateColours {
    /// The colour for one state.
    #[must_use]
    pub const fn of(&self, state: AgentState) -> Colour {
        match state {
            AgentState::Working => self.working,
            AgentState::Waiting => self.waiting,
            AgentState::Done => self.done,
            AgentState::Failed => self.failed,
        }
    }
}

/// Keys that act on whatever fleets are open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Hotkeys {
    /// Focus the terminal that has waited longest for you.
    pub next_waiting: Option<HotkeyText>,
    /// Put every fleet terminal back in its cell.
    pub snap: Option<HotkeyText>,
    /// Close every fleet's terminals at once, ending the agents in them.
    pub close: Option<HotkeyText>,
}

impl Default for Hotkeys {
    /// Ctrl and Alt, because common tiling setups live on Alt alone, and keys
    /// that type nothing with AltGr on the German, Swiss and French layouts,
    /// where AltGr is Ctrl and Alt together. Not Ctrl, Alt and Space: on the
    /// first machine Daifuku ran on, another program already held it. Close
    /// is on F4, as Alt and F4 closes one window.
    fn default() -> Self {
        Self {
            next_waiting: Some(HotkeyText::new("ctrl + alt + n")),
            snap: Some(HotkeyText::new("ctrl + alt + backspace")),
            close: Some(HotkeyText::new("ctrl + alt + f4")),
        }
    }
}

/// One fleet: a set of terminals opened and arranged together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Fleet {
    /// The name `daifuku open <name>` takes. Unique.
    pub name: String,
    /// How many terminals, 1 to 16.
    pub count: u32,
    /// Which monitor the fleet goes on.
    pub monitor: MonitorPick,
    /// The part of the monitor the fleet fills, all of it by default.
    pub area: Area,
    /// Force a grid shape instead of the automatic one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
    /// The folder every terminal starts in. Defaults to your profile folder.
    /// `{n}` is replaced by the terminal's number, for one folder per agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<PathBuf>,
    /// What each terminal runs, `claude` by default. `null` opens a shell.
    /// `{n}` is replaced by the terminal's number, 1 for the first cell.
    pub command: Option<String>,
    /// The Windows Terminal profile to open, by name. Defaults to Windows
    /// Terminal's own default profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Open the terminals as administrator. Needs `"elevated": true`.
    pub admin: bool,
    /// Start PowerShell without the user's profile script.
    pub no_profile: bool,
    /// The key that opens this fleet, or brings it back if it is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<HotkeyText>,
    /// Other fleets, by name, that open or come back whenever this one does,
    /// before it, so this one ends up in front. Their own `with` is not
    /// followed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub with: Vec<String>,
    /// Run each terminal in a group of its own at this priority: everything
    /// it starts runs at it and ends when the terminal closes. Left out, the
    /// terminals run as any other program.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// The account its agents sign in with, by its name under `accounts`.
    /// Left out, they use the one in your profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Keep each terminal with the session it runs, as `daifuku spawn`
    /// keeps a peer, so the fleet opens again with every cell resuming its
    /// last session: after Windows restarts, and after the fleet was closed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub resume: bool,
    /// A markdown file of tasks its agents work through, by full path. Each
    /// time one of them ends a turn holding no task, Daifuku hands it the
    /// next open one, within its account's `stop_at`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backlog: Option<PathBuf>,
}

impl Default for Fleet {
    fn default() -> Self {
        Self {
            name: "agents".to_owned(),
            count: 6,
            monitor: MonitorPick::Portrait,
            area: Area::Full,
            shape: None,
            directory: None,
            command: Some("claude".to_owned()),
            profile: None,
            admin: false,
            no_profile: false,
            hotkey: Some(HotkeyText::new("ctrl + alt + return")),
            with: Vec::new(),
            priority: None,
            account: None,
            resume: false,
            backlog: None,
        }
    }
}

/// Which monitor a fleet goes on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MonitorPick {
    /// The first monitor taller than wide, else the primary one.
    #[default]
    Portrait,
    /// The first monitor wider than tall, preferring the primary one.
    Landscape,
    /// The primary monitor.
    Primary,
    /// The first monitor that is not the primary one, else the primary one.
    Secondary,
    /// The monitor the mouse is on.
    Cursor,
    /// A monitor by its device name, `\\.\DISPLAY2`.
    Device(String),
}

impl FromStr for MonitorPick {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "portrait" => Self::Portrait,
            "landscape" => Self::Landscape,
            "primary" => Self::Primary,
            "secondary" => Self::Secondary,
            "cursor" | "mouse" => Self::Cursor,
            _ => Self::Device(s.to_owned()),
        })
    }
}

impl fmt::Display for MonitorPick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Cursor => "cursor",
            Self::Device(d) => d,
        })
    }
}

impl Serialize for MonitorPick {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MonitorPick {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(s.parse().unwrap_or(Self::Portrait))
    }
}

impl JsonSchema for MonitorPick {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MonitorPick".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // The enum is what an editor offers; the last pattern takes every
        // word the loader takes, in any case, `mouse` included.
        schemars::json_schema!({
            "description": "portrait, landscape, primary, secondary, cursor, or a device name such as \\\\.\\DISPLAY2",
            "anyOf": [
                { "enum": ["portrait", "landscape", "primary", "secondary", "cursor"] },
                { "type": "string", "pattern": r"^\\\\\.\\" },
                { "type": "string", "pattern": any_case(&MONITOR_WORDS) }
            ]
        })
    }
}

/// Every word [`MonitorPick`] reads as a keyword, in lower case.
const MONITOR_WORDS: [&str; 6] = [
    "portrait",
    "landscape",
    "primary",
    "secondary",
    "cursor",
    "mouse",
];

/// A pattern that matches exactly one of `words`, in any case. Written with
/// a class per letter, which every JSON schema validator reads, not with an
/// inline flag, which many do not.
fn any_case(words: &[&str]) -> String {
    let alternatives: Vec<String> = words
        .iter()
        .map(|w| {
            w.chars()
                .map(|c| format!("[{c}{}]", c.to_ascii_uppercase()))
                .collect()
        })
        .collect();
    format!("^({})$", alternatives.join("|"))
}

/// The processor priority a group of processes runs at. Read in any case,
/// as `monitor` and `area` are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    /// Only when nothing else wants the processor, as a build should run
    /// while someone plays or listens to music.
    Idle,
    /// Below other programs.
    BelowNormal,
    /// As any other program.
    Normal,
}

impl Priority {
    /// The name it is written with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::BelowNormal => "below-normal",
            Self::Normal => "normal",
        }
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "idle" => Ok(Self::Idle),
            "below-normal" => Ok(Self::BelowNormal),
            "normal" => Ok(Self::Normal),
            _ => Err(format!(
                "`{s}` is not a priority, write idle, below-normal or normal"
            )),
        }
    }
}

/// The part of its monitor's work area a fleet fills: all of it, a half, or
/// any rectangle given as fractions.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Area {
    /// The whole work area.
    #[default]
    Full,
    /// The left half.
    Left,
    /// The right half.
    Right,
    /// The top half.
    Top,
    /// The bottom half.
    Bottom,
    /// A rectangle of fractions of the work area.
    Part(Part),
}

// JSON has no NaN, so every area a config file holds equals itself.
impl Eq for Area {}

/// A rectangle as fractions of a monitor's work area, from its top left
/// corner: `x` and `width` of its width, `y` and `height` of its height.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// The left edge, 0 to 1.
    pub x: f64,
    /// The top edge, 0 to 1.
    pub y: f64,
    /// The width, more than 0 and at most 1.
    pub width: f64,
    /// The height, more than 0 and at most 1.
    pub height: f64,
}

impl Part {
    /// How far past 1 an edge may add up to and still count as 1: fractions
    /// that add up to 1 on paper can come out a hair over it in floating
    /// point.
    const SLACK: f64 = 1e-9;

    /// What is wrong with it, if anything.
    fn problem(&self) -> Option<String> {
        let all = [self.x, self.y, self.width, self.height];
        if all.iter().any(|v| !(0.0..=1.0).contains(v)) {
            return Some(
                "x, y, width and height must be 0 to 1, as fractions of the monitor".into(),
            );
        }
        if self.width <= 0.0 || self.height <= 0.0 {
            return Some("width and height must be more than 0".into());
        }
        if self.x + self.width > 1.0 + Self::SLACK {
            return Some("x plus width is more than 1, past the right edge of the monitor".into());
        }
        if self.y + self.height > 1.0 + Self::SLACK {
            return Some(
                "y plus height is more than 1, past the bottom edge of the monitor".into(),
            );
        }
        None
    }
}

impl Area {
    /// The area as fractions of the work area.
    #[must_use]
    pub const fn part(self) -> Part {
        let (x, y, width, height) = match self {
            Self::Full => (0.0, 0.0, 1.0, 1.0),
            Self::Left => (0.0, 0.0, 0.5, 1.0),
            Self::Right => (0.5, 0.0, 0.5, 1.0),
            Self::Top => (0.0, 0.0, 1.0, 0.5),
            Self::Bottom => (0.0, 0.5, 1.0, 0.5),
            Self::Part(p) => return p,
        };
        Part {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle the area covers of a monitor's work area `work`.
    #[must_use]
    pub fn within(self, work: Rect) -> Rect {
        let p = self.part();
        work.part(p.x, p.y, p.width, p.height)
    }

    /// The name it is written with, for the four halves and the whole.
    #[must_use]
    pub const fn name(self) -> Option<&'static str> {
        match self {
            Self::Full => Some("full"),
            Self::Left => Some("left"),
            Self::Right => Some("right"),
            Self::Top => Some("top"),
            Self::Bottom => Some("bottom"),
            Self::Part(_) => None,
        }
    }
}

/// Every name [`Area`] reads, in lower case.
const AREA_WORDS: [&str; 5] = ["full", "left", "right", "top", "bottom"];

impl FromStr for Area {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "full" => Self::Full,
            "left" => Self::Left,
            "right" => Self::Right,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            _ => return Err(ConfigError::AreaName(s.to_owned())),
        })
    }
}

impl Serialize for Area {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.name() {
            Some(name) => s.serialize_str(name),
            None => self.part().serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Area {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Area;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "full, left, right, top, bottom, or an object with x, y, width and height",
                )
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Area, E> {
                s.parse().map_err(E::custom)
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Area, A::Error> {
                Part::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Area::Part)
            }
        }

        d.deserialize_any(Visitor)
    }
}

impl JsonSchema for Area {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Area".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let fraction = |description: &str| serde_json::json!({ "description": description, "type": "number", "minimum": 0, "maximum": 1 });
        let size = |description: &str| serde_json::json!({ "description": description, "type": "number", "exclusiveMinimum": 0, "maximum": 1 });
        schemars::json_schema!({
            "description": "full, left, right, top, bottom, or a rectangle as fractions of the monitor's work area",
            "anyOf": [
                { "enum": AREA_WORDS },
                { "type": "string", "pattern": any_case(&AREA_WORDS) },
                {
                    "type": "object",
                    "properties": {
                        "x": fraction("The left edge, 0 to 1."),
                        "y": fraction("The top edge, 0 to 1."),
                        "width": size("The width, more than 0 and at most 1."),
                        "height": size("The height, more than 0 and at most 1.")
                    },
                    "required": ["x", "y", "width", "height"],
                    "additionalProperties": false
                }
            ]
        })
    }
}

/// A hotkey as written in the file. Kept as text so the file round-trips as
/// the user wrote it; [`Config::validate`] proves every one parses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct HotkeyText(pub String);

impl HotkeyText {
    /// Wraps a spelling.
    #[must_use]
    pub fn new(s: &str) -> Self {
        Self(s.to_owned())
    }

    /// Parses it.
    ///
    /// # Errors
    ///
    /// When the text is not a valid hotkey.
    pub fn parse(&self) -> Result<Hotkey, crate::hotkey::HotkeyError> {
        self.0.parse()
    }
}

/// An sRGB colour, written `#rrggbb` in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Colour {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Colour {
    /// A colour from its channels.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `#rrggbb`, lower case.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl FromStr for Colour {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        let bad = || ConfigError::Colour(s.to_owned());
        if hex.len() != 6 || !hex.is_ascii() {
            return Err(bad());
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
        Ok(Self::new(channel(0)?, channel(2)?, channel(4)?))
    }
}

impl Serialize for Colour {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Colour {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Colour {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Colour".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^#?[0-9a-fA-F]{6}$",
            "description": "A colour as #rrggbb"
        })
    }
}

/// Everything that can be wrong with a config file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// Not JSON, or JSON of the wrong shape.
    #[error("the config is not valid: {0}")]
    Parse(String),
    /// A colour that is not `#rrggbb`.
    #[error("`{0}` is not a colour, write it as #rrggbb")]
    Colour(String),
    /// Two fleets with one name.
    #[error("two fleets are called `{0}`")]
    DuplicateFleet(String),
    /// A fleet with no name.
    #[error("a fleet needs a name")]
    UnnamedFleet,
    /// A count outside 1 to 16.
    #[error("fleet `{name}` asks for {count} terminals, the range is 1 to {MAX_FLEET}")]
    Count {
        /// The fleet.
        name: String,
        /// What it asked for.
        count: u32,
    },
    /// A monitor that is neither a keyword nor a device name.
    #[error(
        "fleet `{name}`: monitor `{text}` is not portrait, landscape, primary, secondary, cursor or a device name such as \\\\.\\DISPLAY2"
    )]
    Monitor {
        /// The fleet.
        name: String,
        /// As written.
        text: String,
    },
    /// An area name that is not one of the halves or `full`.
    #[error(
        "`{0}` is not an area, write full, left, right, top, bottom or {{ \"x\", \"y\", \"width\", \"height\" }}"
    )]
    AreaName(String),
    /// An area that does not fit on its monitor.
    #[error("fleet `{name}`: area {reason}")]
    Area {
        /// The fleet.
        name: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A priority for a fleet whose terminals run no command, so no group
    /// to run at it.
    #[error(
        "fleet `{0}` has a priority but no command: a priority applies to the command a terminal runs"
    )]
    PriorityWithoutCommand(String),
    /// A fleet that resumes sessions but runs no command to resume them in.
    #[error(
        "fleet `{0}` resumes sessions but has no command: a session resumes in the command a terminal runs"
    )]
    ResumeWithoutCommand(String),
    /// A hotkey that does not parse.
    #[error("hotkey `{text}`: {reason}")]
    Hotkey {
        /// As written.
        text: String,
        /// Why it failed.
        reason: String,
    },
    /// One combination bound twice.
    #[error("`{0}` is bound twice")]
    DuplicateHotkey(String),
    /// A fleet to open with another that is not in the file, or is that
    /// fleet itself.
    #[error("fleet `{name}` opens `{other}` with it, which is {reason}")]
    With {
        /// The fleet.
        name: String,
        /// The one it names.
        other: String,
        /// `itself`, or `not a fleet in the file`.
        reason: &'static str,
    },
    /// Gaps or border sizes that make no sense.
    #[error("{0}")]
    Size(String),
    /// A fleet's account that is not under `accounts`.
    #[error("fleet `{name}` signs in with account `{account}`, which is not under accounts")]
    UnknownAccount {
        /// The fleet.
        name: String,
        /// The account it names.
        account: String,
    },
    /// A fleet's backlog that is not a full path.
    #[error(
        "fleet `{0}`: backlog is not a full path, write it from the drive, such as C:\\work\\BACKLOG.md"
    )]
    Backlog(String),
    /// An account that cannot be told apart from another, names no folder,
    /// or names one that is not a full path.
    #[error("account `{name}`: {reason}")]
    Account {
        /// The account.
        name: String,
        /// What is wrong with it.
        reason: String,
    },
}

/// A config file's bytes as text: UTF-8, or UTF-16 when the file starts with
/// its byte order mark, as Windows PowerShell's `>` and `Out-File` write it.
///
/// # Errors
///
/// `InvalidData` when the bytes are neither.
pub fn decode(bytes: &[u8]) -> std::io::Result<String> {
    let invalid = |e: String| std::io::Error::new(std::io::ErrorKind::InvalidData, e);
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let (pairs, _) = rest.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|&pair| unit(pair)).collect();
        String::from_utf16(&units)
            .map_err(|_| invalid("the file is not whole UTF-16 text: save it as UTF-8".to_owned()))
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => String::from_utf8(bytes.to_vec()).map_err(|e| {
            let good = &bytes[..e.utf8_error().valid_up_to()];
            let line = good.iter().filter(|&&b| b == b'\n').count() + 1;
            invalid(format!(
                "line {line} is not UTF-8 text: save the file as UTF-8"
            ))
        }),
    }
}

/// What is wrong with a file that did not parse, in the words of someone
/// editing it rather than the parser's, with where it is.
fn plain_json_error(e: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let full = e.to_string();
    let place = format!(" at line {} column {}", e.line(), e.column());
    let message = full.strip_suffix(&place).unwrap_or(&full);
    let words = match e.classify() {
        Category::Eof => "the file ends before every {, [ and \" in it is closed".to_owned(),
        Category::Syntax => plain_syntax(message).to_owned(),
        Category::Data => plain_data(message),
        Category::Io => message.to_owned(),
    };
    if e.line() == 0 {
        words
    } else {
        format!("{words}{place}")
    }
}

/// A JSON syntax error in plain words.
fn plain_syntax(message: &str) -> &str {
    match message {
        "key must be a string" => "a key must be in double quotes, and comments are not allowed",
        "trailing comma" => "there is a comma after the last item",
        "expected `,` or `}`" => "a comma or a } is missing",
        "expected `,` or `]`" => "a comma or a ] is missing",
        "expected `:`" => "a colon is missing after a key",
        "expected value" | "expected ident" => {
            "a value is missing or misspelt; text goes in double quotes, and true, false and null in lower case"
        }
        "invalid escape" => r#"a backslash must be written twice, as in "C:\\work""#,
        "control character (\\u0000-\\u001F) found while parsing a string" => {
            r"a line break or tab inside double quotes must be written as \n or \t"
        }
        "trailing characters" => "there is more after the last }",
        "invalid number" | "number out of range" => "a number does not read as one",
        other => other,
    }
}

/// A value of the wrong kind in plain words. Messages the config's own
/// types give are plain already and stay as they are.
fn plain_data(message: &str) -> String {
    let wrong = message
        .strip_prefix("invalid type: ")
        .or_else(|| message.strip_prefix("invalid value: "))
        .and_then(|rest| rest.rsplit_once(", expected "));
    if let Some((got, want)) = wrong {
        return format!("expected {}, not {}", plain_kind(want), plain_value(got));
    }
    if let Some(rest) = message.strip_prefix("unknown field ") {
        return format!("unknown key {rest}");
    }
    if let Some(name) = message.strip_prefix("missing field ") {
        return format!("{name} is missing");
    }
    if let Some(name) = message.strip_prefix("duplicate field ") {
        return format!("{name} is there twice");
    }
    message.to_owned()
}

/// What a value should have been, for a person rather than a programmer.
fn plain_kind(want: &str) -> &str {
    match want {
        "u8" | "u16" | "u32" | "u64" | "usize" => "a whole number from 0",
        "i8" | "i16" | "i32" | "i64" | "isize" => "a whole number",
        "f32" | "f64" => "a number",
        "a boolean" => "true or false",
        "a string" | "a borrowed string" | "a character" => "text in double quotes",
        "a sequence" => "a list in [ ]",
        "a map" => "an object in { }",
        w if w.starts_with("struct ") => "an object in { }",
        w => w,
    }
}

/// The value that was there instead, as it is written in the file.
fn plain_value(got: &str) -> String {
    let quoted = |prefix: &str| {
        got.strip_prefix(prefix)
            .and_then(|v| v.strip_prefix('`'))
            .and_then(|v| v.strip_suffix('`'))
            .map(str::to_owned)
    };
    if let Some(text) = got.strip_prefix("string ") {
        return text.to_owned();
    }
    quoted("integer ")
        .or_else(|| quoted("floating point "))
        .or_else(|| quoted("boolean "))
        .unwrap_or_else(|| {
            match got {
                "map" => "an object",
                "sequence" => "a list",
                "unit value" => "null",
                other => other,
            }
            .to_owned()
        })
}

impl Config {
    /// Reads and validates a config file's text. An empty or whitespace-only
    /// file is the default config. A byte order mark, which Notepad and other
    /// Windows editors may write, is skipped.
    ///
    /// # Errors
    ///
    /// When the JSON does not parse or [`Config::validate`] fails.
    pub fn from_json(text: &str) -> Result<Self, ConfigError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let config = if text.trim().is_empty() {
            Self::default()
        } else {
            serde_json::from_str(text).map_err(|e| ConfigError::Parse(plain_json_error(&e)))?
        };
        config.validate()?;
        Ok(config)
    }

    /// Checks everything a type cannot: unique names, counts in range, every
    /// monitor a keyword or a device name, every area on its monitor, every
    /// fleet opened with another one in the file, every hotkey parses and
    /// none is bound twice.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(0..=1000).contains(&self.gaps.outer) || !(0..=1000).contains(&self.gaps.inner) {
            return Err(ConfigError::Size("gaps must be 0 to 1000 pixels".into()));
        }
        if !(0..=64).contains(&self.border.width) {
            return Err(ConfigError::Size(
                "border width must be 1 to 64 pixels; set enabled to false for no border".into(),
            ));
        }
        // Past this a border's bitmap grows without bound, and near the end
        // of the integer range its size wraps.
        if !(-64..=64).contains(&self.border.offset) {
            return Err(ConfigError::Size(
                "border offset must be -64 to 64 pixels".into(),
            ));
        }
        let mut names = BTreeSet::new();
        for fleet in &self.fleets {
            if fleet.name.trim().is_empty() {
                return Err(ConfigError::UnnamedFleet);
            }
            if !names.insert(fleet.name.to_ascii_lowercase()) {
                return Err(ConfigError::DuplicateFleet(fleet.name.clone()));
            }
            if !(1..=MAX_FLEET).contains(&fleet.count) {
                return Err(ConfigError::Count {
                    name: fleet.name.clone(),
                    count: fleet.count,
                });
            }
            // Windows names every monitor `\\.\DISPLAYn`. Anything else is a
            // misspelt keyword, which would open the fleet on the primary
            // monitor without a word.
            if let MonitorPick::Device(text) = &fleet.monitor
                && !text.starts_with(r"\\.\")
            {
                return Err(ConfigError::Monitor {
                    name: fleet.name.clone(),
                    text: text.clone(),
                });
            }
            if fleet.priority.is_some() && fleet.command.is_none() {
                return Err(ConfigError::PriorityWithoutCommand(fleet.name.clone()));
            }
            if fleet.resume && fleet.command.is_none() {
                return Err(ConfigError::ResumeWithoutCommand(fleet.name.clone()));
            }
            if let Some(reason) = fleet.area.part().problem() {
                return Err(ConfigError::Area {
                    name: fleet.name.clone(),
                    reason,
                });
            }
            if fleet.backlog.as_deref().is_some_and(|b| !b.is_absolute()) {
                return Err(ConfigError::Backlog(fleet.name.clone()));
            }
            if let Some(account) = &fleet.account
                && self.account(account).is_none()
            {
                return Err(ConfigError::UnknownAccount {
                    name: fleet.name.clone(),
                    account: account.clone(),
                });
            }
        }
        self.validate_accounts()?;
        for fleet in &self.fleets {
            for other in &fleet.with {
                let reason = if other.eq_ignore_ascii_case(&fleet.name) {
                    "itself"
                } else if self.fleet(other).is_none() {
                    "not a fleet in the file"
                } else {
                    continue;
                };
                return Err(ConfigError::With {
                    name: fleet.name.clone(),
                    other: other.clone(),
                    reason,
                });
            }
        }
        let mut bound = BTreeSet::new();
        for text in self.all_hotkeys() {
            let key = text.parse().map_err(|e| ConfigError::Hotkey {
                text: text.0.clone(),
                reason: e.to_string(),
            })?;
            if !bound.insert(key) {
                return Err(ConfigError::DuplicateHotkey(key.to_string()));
            }
        }
        Ok(())
    }

    /// Every account has a name of its own in any case, at least one folder,
    /// and only full paths: a relative one would be read from whatever
    /// folder the terminal starts in.
    fn validate_accounts(&self) -> Result<(), ConfigError> {
        let mut names = BTreeSet::new();
        // Each variable's folder, as written in any case and with or
        // without a separator at the end: two accounts in one folder are
        // one sign-in.
        let mut folders = BTreeSet::new();
        for (name, account) in &self.accounts {
            let problem = |reason: &str| ConfigError::Account {
                name: name.clone(),
                reason: reason.to_owned(),
            };
            if name.trim().is_empty() {
                return Err(problem("an account needs a name"));
            }
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(problem("two accounts have this name"));
            }
            let variables = account.variables();
            if variables.is_empty() {
                return Err(problem("name a folder for claude, codex or both"));
            }
            if account.warn.is_some_and(|w| !(1..=100).contains(&w)) {
                return Err(problem("warn is a percentage from 1 to 100"));
            }
            if let Some(reason) = account.stop_at.iter().find_map(StopAt::problem) {
                return Err(problem(&reason));
            }
            for (variable, folder) in variables {
                if !folders.insert((variable, folder_key(folder))) {
                    return Err(ConfigError::Account {
                        name: name.clone(),
                        reason: format!("`{}` is another account's folder too", folder.display()),
                    });
                }
                if !folder.is_absolute() {
                    return Err(ConfigError::Account {
                        name: name.clone(),
                        reason: format!(
                            "`{}` is not a full path, write it from the drive, such as C:\\Users\\you\\.claude-work",
                            folder.display()
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// An account by name, ignoring case.
    #[must_use]
    pub fn account(&self, name: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, a)| a)
    }

    /// The account `fleet` signs in with, `None` for the one in the profile.
    #[must_use]
    pub fn account_of(&self, fleet: &Fleet) -> Option<&Account> {
        fleet.account.as_deref().and_then(|a| self.account(a))
    }

    /// The account whose Claude Code folder is `folder`, in any case and
    /// with or without a separator at the end, with its name.
    #[must_use]
    pub fn account_in(&self, folder: &Path) -> Option<(&str, &Account)> {
        let key = folder_key(folder);
        self.accounts
            .iter()
            .find(|(_, a)| a.claude.as_deref().is_some_and(|c| folder_key(c) == key))
            .map(|(n, a)| (n.as_str(), a))
    }

    fn all_hotkeys(&self) -> impl Iterator<Item = &HotkeyText> {
        self.hotkeys
            .next_waiting
            .iter()
            .chain(self.hotkeys.snap.iter())
            .chain(self.hotkeys.close.iter())
            .chain(self.fleets.iter().filter_map(|f| f.hotkey.as_ref()))
    }

    /// A fleet by name, ignoring case.
    #[must_use]
    pub fn fleet(&self, name: &str) -> Option<&Fleet> {
        self.fleets
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// The JSON schema editors validate the file against.
    #[must_use]
    pub fn schema() -> String {
        let schema = schemars::schema_for!(Config);
        serde_json::to_string_pretty(&schema).unwrap_or_default() + "\n"
    }
}

/// A folder as accounts are told apart by: each part in lower case, so
/// `C:\Users\Me\.claude\` and `c:\users\me\.claude` are one.
#[must_use]
pub fn folder_key(folder: &Path) -> Vec<String> {
    folder
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_monitor_keyword_parses_to_its_own_pick() {
        assert_eq!(
            "portrait".parse::<MonitorPick>().unwrap(),
            MonitorPick::Portrait
        );
        assert_eq!(
            "landscape".parse::<MonitorPick>().unwrap(),
            MonitorPick::Landscape
        );
        assert_eq!(
            "primary".parse::<MonitorPick>().unwrap(),
            MonitorPick::Primary
        );
        assert_eq!(
            "secondary".parse::<MonitorPick>().unwrap(),
            MonitorPick::Secondary
        );
        assert_eq!(
            "cursor".parse::<MonitorPick>().unwrap(),
            MonitorPick::Cursor
        );
        assert_eq!(
            "DISPLAY9".parse::<MonitorPick>().unwrap(),
            MonitorPick::Device("DISPLAY9".into())
        );
    }

    #[test]
    fn the_schema_describes_monitors_and_colours() {
        let schema: serde_json::Value = serde_json::from_str(&Config::schema()).unwrap();
        let defs = &schema["$defs"];
        assert!(defs["MonitorPick"]["anyOf"].is_array());
        // Any other string is a misspelt keyword, which editors should flag.
        assert_eq!(defs["MonitorPick"]["anyOf"][1]["pattern"], r"^\\\\\.\\");
        assert_eq!(defs["Colour"]["pattern"], "^#?[0-9a-fA-F]{6}$");
    }

    /// Whether `text` matches `pattern`, a pattern as [`any_case`] writes
    /// it: alternatives of one bracketed class per letter.
    fn matches_any_case(pattern: &str, text: &str) -> bool {
        let inner = pattern
            .strip_prefix("^(")
            .and_then(|p| p.strip_suffix(")$"))
            .unwrap();
        inner.split('|').any(|alternative| {
            let classes: Vec<&str> = alternative
                .split(']')
                .filter(|c| !c.is_empty())
                .map(|c| c.strip_prefix('[').unwrap())
                .collect();
            classes.len() == text.chars().count()
                && classes.iter().zip(text.chars()).all(|(c, t)| c.contains(t))
        })
    }

    #[test]
    fn the_schema_takes_every_monitor_word_the_loader_takes() {
        let schema: serde_json::Value = serde_json::from_str(&Config::schema()).unwrap();
        let pattern = schema["$defs"]["MonitorPick"]["anyOf"][2]["pattern"]
            .as_str()
            .unwrap();
        for text in [
            "Portrait",
            "mouse",
            "MOUSE",
            "Cursor",
            "landscape",
            "SECONDARY",
        ] {
            let json = format!(r#"{{"fleets":[{{"name":"a","monitor":"{text}"}}]}}"#);
            assert!(Config::from_json(&json).is_ok(), "the loader takes {text}");
            assert!(matches_any_case(pattern, text), "the schema takes {text}");
        }
        for text in ["sideways", "Wide", "DISPLAY2", "portraits", "mous"] {
            assert!(!matches_any_case(pattern, text), "{text}");
        }
        for word in MONITOR_WORDS {
            assert!(
                !matches!(word.parse::<MonitorPick>(), Ok(MonitorPick::Device(_))),
                "{word} is a keyword to the loader"
            );
        }
    }

    #[test]
    fn zero_gaps_are_allowed() {
        assert!(Config::from_json(r#"{"gaps":{"outer":0,"inner":0}}"#).is_ok());
        assert!(Config::from_json(r#"{"gaps":{"outer":0,"inner":-1}}"#).is_err());
    }

    #[test]
    fn no_file_is_one_fleet_of_six_claudes_on_the_portrait_monitor_with_the_users_rights() {
        let c = Config::from_json("").unwrap();
        assert!(!c.elevated);
        assert_eq!(c.fleets.len(), 1);
        let f = &c.fleets[0];
        assert_eq!(
            (f.count, f.admin, f.command.as_deref()),
            (6, false, Some("claude"))
        );
        assert_eq!(f.monitor, MonitorPick::Portrait);
    }

    #[test]
    fn elevation_is_asked_for_by_one_top_level_key() {
        assert!(Config::from_json(r#"{"elevated": true}"#).unwrap().elevated);
        assert!(
            !Config::from_json(r#"{"elevated": false}"#)
                .unwrap()
                .elevated
        );
        assert!(Config::from_json(r#"{"elevated": "yes"}"#).is_err());
        // An administrator fleet alone does not ask for it.
        let admin = Config::from_json(r#"{"fleets":[{"name":"a","admin":true}]}"#).unwrap();
        assert!(admin.fleets[0].admin && !admin.elevated);
    }

    #[test]
    fn a_utf16_file_reads_like_a_utf8_one() {
        let text = "{\"fleets\":[{\"name\":\"ä\"}]}";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        for bytes in [le, be, text.as_bytes().to_vec()] {
            let c = Config::from_json(&decode(&bytes).unwrap()).unwrap();
            assert_eq!(c.fleets[0].name, "ä");
        }
        assert!(decode(&[0xC3]).is_err());
    }

    #[test]
    fn a_file_in_another_encoding_says_which_line_in_plain_words() {
        let ansi = b"{\"fleets\": [\n  {\"name\": \"Gr\xfcezi\"}\n]}";
        let said = decode(ansi).unwrap_err().to_string();
        assert_eq!(said, "line 2 is not UTF-8 text: save the file as UTF-8");
        let mut lone = vec![0xFF, 0xFE];
        lone.extend(0xD800u16.to_le_bytes());
        let said = decode(&lone).unwrap_err().to_string();
        assert_eq!(said, "the file is not whole UTF-16 text: save it as UTF-8");
    }

    #[test]
    fn a_byte_order_mark_is_skipped() {
        let c = Config::from_json("\u{feff}{\"fleets\":[{\"name\":\"a\"}]}").unwrap();
        assert_eq!(c.fleets[0].name, "a");
        assert!(Config::from_json("\u{feff}").is_ok());
    }

    #[test]
    fn an_empty_object_is_the_default_too() {
        assert_eq!(Config::from_json("{}").unwrap(), Config::default());
    }

    #[test]
    fn the_example_in_the_module_docs_parses() {
        let text = r#"{
          "$schema": "https://get-daifuku.vercel.app/schema.json",
          "fleets": [
            { "name": "agents", "count": 6, "monitor": "portrait", "hotkey": "ctrl + alt + return" },
            { "name": "mochi", "count": 4, "directory": "C:\\src\\Mochi", "hotkey": "ctrl + alt + m" }
          ]
        }"#;
        let c = Config::from_json(text).unwrap();
        assert_eq!(c.fleet("MOCHI").unwrap().count, 4);
        assert_eq!(c.fleet("mochi").unwrap().command.as_deref(), Some("claude"));
    }

    #[test]
    fn a_null_command_means_a_plain_shell() {
        let c = Config::from_json(r#"{"fleets":[{"name":"x","command":null}]}"#).unwrap();
        assert_eq!(c.fleets[0].command, None);
    }

    #[test]
    fn a_typo_in_a_key_is_an_error_not_a_silent_default() {
        let e = Config::from_json(r#"{"fleets":[{"name":"x","cuont":3}]}"#).unwrap_err();
        assert!(matches!(e, ConfigError::Parse(_)), "{e}");
    }

    #[test]
    fn a_file_that_does_not_parse_says_why_in_plain_words() {
        let cases = [
            (
                r#"{"fleets":[{"name":"a","count":"four"}]}"#,
                "a whole number",
            ),
            (r#"{"fleets":[{"name":"a","count":-1}]}"#, "a whole number"),
            (r#"{"border":{"width":1.5}}"#, "a whole number"),
            (r#"{"fleets":["a"]}"#, "an object"),
            (r#"{"border":{"pulse":"yes"}}"#, "true or false"),
            ("{\n  \"fleets\": [{ \"name\": \"a\" }\n", "ends before"),
            ("{\n  // mine\n  \"fleets\": []\n}", "comments"),
            (r#"{"fleets":[{"name":"C:\work"}]}"#, "backslash"),
            (r#"{"fleets":[{"name":"a" "count":2}]}"#, "comma"),
        ];
        for (text, words) in cases {
            let message = Config::from_json(text).unwrap_err().to_string();
            assert!(message.contains(words), "{text}: {message}");
            assert!(message.contains(" at line "), "{text}: {message}");
            for tell in [
                "u32",
                "i32",
                "EOF",
                "invalid type",
                "invalid value",
                "struct",
                "must be a string",
                "floating point",
                "integer `",
                "string \"",
            ] {
                assert!(!message.contains(tell), "{text}: {message}");
            }
        }
    }

    #[test]
    fn rejects_duplicate_fleet_names_ignoring_case() {
        let e = Config::from_json(
            r#"{"fleets":[{"name":"a","hotkey":null},{"name":"A","hotkey":null}]}"#,
        );
        assert_eq!(e, Err(ConfigError::DuplicateFleet("A".into())));
    }

    #[test]
    fn rejects_counts_out_of_range() {
        for count in [0, 17] {
            let text = format!(r#"{{"fleets":[{{"name":"a","count":{count}}}]}}"#);
            assert!(matches!(
                Config::from_json(&text),
                Err(ConfigError::Count { .. })
            ));
        }
    }

    #[test]
    fn rejects_a_hotkey_bound_twice_in_any_spelling() {
        let text = r#"{"hotkeys":{"snap":"alt + ctrl + enter"}}"#;
        assert_eq!(
            Config::from_json(text),
            Err(ConfigError::DuplicateHotkey("ctrl + alt + return".into()))
        );
    }

    #[test]
    fn rejects_a_hotkey_that_does_not_parse() {
        let e = Config::from_json(r#"{"hotkeys":{"snap":"ctrl + banana"}}"#).unwrap_err();
        assert!(matches!(e, ConfigError::Hotkey { .. }), "{e}");
    }

    #[test]
    fn the_close_key_is_on_f4_until_turned_off() {
        let key = Config::default().hotkeys.close.unwrap().parse().unwrap();
        assert_eq!(key.to_string(), "ctrl + alt + f4");
        let c = Config::from_json(r#"{"hotkeys":{"close":null}}"#).unwrap();
        assert_eq!(c.hotkeys.close, None);
    }

    #[test]
    fn colours_parse_with_or_without_the_hash_and_print_canonically() {
        assert_eq!(
            "#FFBBDF".parse::<Colour>().unwrap(),
            Colour::new(0xff, 0xbb, 0xdf)
        );
        assert_eq!("ffbbdf".parse::<Colour>().unwrap().to_hex(), "#ffbbdf");
        for bad in ["#fff", "#gggggg", "#ffbbdf00", "#ffbbd\u{e9}"] {
            assert!(bad.parse::<Colour>().is_err(), "{bad}");
        }
    }

    #[test]
    fn monitor_picks_round_trip() {
        for s in [
            "portrait",
            "landscape",
            "primary",
            "secondary",
            "cursor",
            r"\\.\DISPLAY2",
        ] {
            let pick: MonitorPick = s.parse().unwrap();
            assert_eq!(pick.to_string(), s);
        }
        assert_eq!("Mouse".parse::<MonitorPick>().unwrap(), MonitorPick::Cursor);
    }

    #[test]
    fn a_misspelt_monitor_is_an_error_not_the_primary_monitor() {
        for text in ["sideways", "Wide", "DISPLAY2"] {
            let json = format!(r#"{{"fleets":[{{"name":"a","monitor":"{text}"}}]}}"#);
            assert_eq!(
                Config::from_json(&json),
                Err(ConfigError::Monitor {
                    name: "a".into(),
                    text: text.into()
                }),
                "{text}"
            );
        }
        for text in ["Portrait", "mouse", r"\\\\.\\DISPLAY2", r"\\\\.\\display2"] {
            let json = format!(r#"{{"fleets":[{{"name":"a","monitor":"{text}"}}]}}"#);
            assert!(Config::from_json(&json).is_ok(), "{text}");
        }
    }

    fn area_of(area: &str) -> Result<Area, ConfigError> {
        let json = format!(r#"{{"fleets":[{{"name":"a","area":{area}}}]}}"#);
        Config::from_json(&json).map(|c| c.fleets[0].area)
    }

    fn part(x: f64, y: f64, width: f64, height: f64) -> Area {
        Area::Part(Part {
            x,
            y,
            width,
            height,
        })
    }

    #[test]
    fn a_fleet_fills_its_whole_monitor_unless_told_otherwise() {
        let c = Config::from_json(r#"{"fleets":[{"name":"a"}]}"#).unwrap();
        assert_eq!(c.fleets[0].area, Area::Full);
        assert_eq!(Fleet::default().area, Area::Full);
    }

    #[test]
    fn every_area_name_parses_in_any_case() {
        for (text, area) in [
            ("full", Area::Full),
            ("left", Area::Left),
            ("Right", Area::Right),
            ("TOP", Area::Top),
            ("bottom", Area::Bottom),
        ] {
            assert_eq!(area_of(&format!(r#""{text}""#)), Ok(area), "{text}");
        }
    }

    #[test]
    fn an_area_of_fractions_parses_whole_numbers_too() {
        assert_eq!(
            area_of(r#"{ "x": 0.5, "y": 0, "width": 0.5, "height": 1 }"#),
            Ok(part(0.5, 0.0, 0.5, 1.0))
        );
    }

    #[test]
    fn areas_round_trip_as_written() {
        for (area, text) in [
            (Area::Full, r#""full""#),
            (Area::Left, r#""left""#),
            (Area::Right, r#""right""#),
            (Area::Top, r#""top""#),
            (Area::Bottom, r#""bottom""#),
            (
                part(0.25, 0.0, 0.5, 1.0),
                r#"{"x":0.25,"y":0.0,"width":0.5,"height":1.0}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&area).unwrap(), text);
            assert_eq!(area_of(text), Ok(area), "{text}");
        }
    }

    #[test]
    fn the_halves_are_the_fractions_they_name() {
        assert_eq!(
            Area::Full.part(),
            Part {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0
            }
        );
        assert_eq!(
            Area::Left.part(),
            Part {
                x: 0.0,
                y: 0.0,
                width: 0.5,
                height: 1.0
            }
        );
        assert_eq!(
            Area::Right.part(),
            Part {
                x: 0.5,
                y: 0.0,
                width: 0.5,
                height: 1.0
            }
        );
        assert_eq!(
            Area::Top.part(),
            Part {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 0.5
            }
        );
        assert_eq!(
            Area::Bottom.part(),
            Part {
                x: 0.0,
                y: 0.5,
                width: 1.0,
                height: 0.5
            }
        );
    }

    #[test]
    fn a_misspelt_area_name_is_an_error_that_names_it() {
        for text in ["middle", "half", "sideways", "", "left half"] {
            let e = area_of(&format!(r#""{text}""#)).unwrap_err();
            let ConfigError::Parse(message) = &e else {
                panic!("{text}: {e}");
            };
            assert!(
                message.contains(&format!("`{text}` is not an area")),
                "{message}"
            );
        }
    }

    #[test]
    fn an_area_of_the_wrong_shape_is_an_error() {
        for text in [
            r#"{ "x": 0.5, "y": 0, "width": 0.5 }"#,
            r#"{ "x": 0.5, "y": 0, "width": 0.5, "height": 1, "z": 0 }"#,
            r#"{ "x": "half", "y": 0, "width": 0.5, "height": 1 }"#,
            "0.5",
            "true",
            "[0.5, 0, 0.5, 1]",
            "null",
        ] {
            assert!(
                matches!(area_of(text), Err(ConfigError::Parse(_))),
                "{text}"
            );
        }
    }

    #[test]
    fn an_area_of_the_wrong_type_says_what_an_area_is() {
        let e = area_of("5").unwrap_err();
        assert!(
            e.to_string().contains(
                "expected full, left, right, top, bottom, or an object with x, y, width and height"
            ),
            "{e}"
        );
    }

    #[test]
    fn an_area_covers_its_part_of_the_work_area() {
        let work = Rect::new(100, 0, 300, 50);
        assert_eq!(Area::Full.within(work), work);
        assert_eq!(Area::Right.within(work), Rect::new(200, 0, 300, 50));
        assert_eq!(
            part(0.25, 0.2, 0.5, 0.6).within(work),
            Rect::new(150, 10, 250, 40)
        );
    }

    #[test]
    fn an_area_off_its_monitor_is_an_error_that_names_the_fleet() {
        for (text, says) in [
            (
                r#"{ "x": -0.1, "y": 0, "width": 0.5, "height": 1 }"#,
                "0 to 1",
            ),
            (
                r#"{ "x": 0, "y": 1.5, "width": 0.5, "height": 0.5 }"#,
                "0 to 1",
            ),
            (r#"{ "x": 0, "y": 0, "width": 2, "height": 1 }"#, "0 to 1"),
            (r#"{ "x": 0, "y": 0, "width": 1, "height": -1 }"#, "0 to 1"),
            (
                r#"{ "x": 0, "y": 0, "width": 0, "height": 1 }"#,
                "more than 0",
            ),
            (
                r#"{ "x": 0, "y": 0, "width": 1, "height": 0 }"#,
                "more than 0",
            ),
            (
                r#"{ "x": 0.6, "y": 0, "width": 0.5, "height": 1 }"#,
                "right edge",
            ),
            (
                r#"{ "x": 0, "y": 0.5, "width": 1, "height": 0.75 }"#,
                "bottom edge",
            ),
        ] {
            let e = area_of(text).unwrap_err();
            assert!(
                matches!(&e, ConfigError::Area { name, reason } if name == "a" && reason.contains(says)),
                "{text}: {e}"
            );
            assert!(e.to_string().starts_with("fleet `a`: area "), "{e}");
        }
    }

    #[test]
    fn an_area_may_end_on_the_far_edges() {
        for text in [
            r#"{ "x": 0, "y": 0, "width": 1, "height": 1 }"#,
            r#"{ "x": 0.7, "y": 0.3, "width": 0.3, "height": 0.7 }"#,
            r#"{ "x": 0.6667, "y": 0, "width": 0.3333, "height": 1 }"#,
            r#"{ "x": 0.1, "y": 0.2, "width": 0.9, "height": 0.8 }"#,
            r#"{ "x": 0.999, "y": 0.999, "width": 0.001, "height": 0.001 }"#,
        ] {
            assert!(area_of(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn an_edge_one_slack_past_the_monitor_still_ends_on_it() {
        let edge = 1.0 + Part::SLACK;
        let across = Part {
            x: edge - 0.5,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        };
        assert_eq!(across.x + across.width, edge);
        assert_eq!(across.problem(), None);
        let down = Part {
            x: 0.0,
            y: edge - 0.5,
            width: 1.0,
            height: 0.5,
        };
        assert_eq!(down.y + down.height, edge);
        assert_eq!(down.problem(), None);
    }

    #[test]
    fn the_schema_takes_every_area_name_the_loader_takes() {
        let schema: serde_json::Value = serde_json::from_str(&Config::schema()).unwrap();
        let area = &schema["$defs"]["Area"]["anyOf"];
        assert_eq!(area[0]["enum"], serde_json::json!(AREA_WORDS));
        let pattern = area[1]["pattern"].as_str().unwrap();
        for text in ["full", "Left", "RIGHT", "Top", "bottom"] {
            assert!(
                area_of(&format!(r#""{text}""#)).is_ok(),
                "the loader takes {text}"
            );
            assert!(matches_any_case(pattern, text), "the schema takes {text}");
        }
        for text in ["middle", "fulls", "half", "lefts"] {
            assert!(!matches_any_case(pattern, text), "{text}");
            assert!(text.parse::<Area>().is_err(), "{text}");
        }
        let object = &area[2];
        assert_eq!(
            object["required"],
            serde_json::json!(["x", "y", "width", "height"])
        );
        assert_eq!(object["additionalProperties"], false);
        assert_eq!(
            schema["$defs"]["Fleet"]["properties"]["area"]["default"],
            "full"
        );
    }

    #[test]
    fn a_fleet_opens_others_with_it_by_name_in_any_case() {
        let c = Config::from_json(
            r#"{"fleets":[{"name":"lead","with":["CREW"]},{"name":"crew","hotkey":null}]}"#,
        )
        .unwrap();
        assert_eq!(c.fleets[0].with, ["CREW"]);
        assert!(c.fleets[1].with.is_empty());
        let text = serde_json::to_string(&c.fleets[1]).unwrap();
        assert!(!text.contains("with"), "{text}");
    }

    #[test]
    fn a_fleet_cannot_open_itself_or_one_that_is_not_there() {
        for (with, reason) in [("Lead", "itself"), ("crow", "not a fleet in the file")] {
            let json = format!(
                r#"{{"fleets":[{{"name":"lead","with":["{with}"]}},{{"name":"crew","hotkey":null}}]}}"#
            );
            assert_eq!(
                Config::from_json(&json),
                Err(ConfigError::With {
                    name: "lead".into(),
                    other: with.into(),
                    reason
                })
            );
        }
        let e = Config::from_json(r#"{"fleets":[{"name":"lead","with":["crow"]}]}"#).unwrap_err();
        assert_eq!(
            e.to_string(),
            "fleet `lead` opens `crow` with it, which is not a fleet in the file"
        );
    }

    #[test]
    fn a_priority_is_one_of_three_words() {
        for p in [Priority::Idle, Priority::BelowNormal, Priority::Normal] {
            assert_eq!(p.name().parse::<Priority>(), Ok(p));
            let text = format!(r#"{{"fleets":[{{"name":"a","priority":"{}"}}]}}"#, p.name());
            assert_eq!(
                Config::from_json(&text).unwrap().fleets[0].priority,
                Some(p)
            );
        }
        assert_eq!("IDLE".parse::<Priority>(), Ok(Priority::Idle));
        let any_case = Config::from_json(r#"{"fleets":[{"name":"a","priority":"Below-Normal"}]}"#);
        assert_eq!(
            any_case.unwrap().fleets[0].priority,
            Some(Priority::BelowNormal)
        );
        assert_eq!(
            Config::from_json(r#"{"fleets":[{"name":"a","priority":"idle","command":null}]}"#),
            Err(ConfigError::PriorityWithoutCommand("a".into()))
        );
        assert_eq!(
            Config::from_json(r#"{"fleets":[{"name":"a","resume":true,"command":null}]}"#),
            Err(ConfigError::ResumeWithoutCommand("a".into()))
        );
        assert!(
            Config::from_json(r#"{"fleets":[{"name":"a","resume":true}]}"#)
                .unwrap()
                .fleets[0]
                .resume
        );
        assert!("low".parse::<Priority>().is_err());
        assert!(matches!(
            Config::from_json(r#"{"fleets":[{"name":"a","priority":"low"}]}"#),
            Err(ConfigError::Parse(_))
        ));
        assert_eq!(Fleet::default().priority, None);
    }

    #[test]
    fn every_state_has_its_own_width() {
        let b = Border::default();
        let widths: Vec<_> = AgentState::ALL.iter().map(|&s| b.width_for(s)).collect();
        assert_eq!(
            widths,
            vec![2, 4, 6, 8],
            "done, working, failed, waiting at width 4"
        );
        let flat = Border {
            state_widths: false,
            ..Border::default()
        };
        assert!(AgentState::ALL.iter().all(|&s| flat.width_for(s) == 4));
        for width in 1..=64 {
            let b = Border {
                width,
                ..Border::default()
            };
            let widths: Vec<_> = AgentState::ALL.iter().map(|&s| b.width_for(s)).collect();
            assert!(widths[0] >= 1, "width {width}: {widths:?}");
            assert!(
                widths.windows(2).all(|w| w[0] < w[1]),
                "width {width}: {widths:?}"
            );
        }
        let b = Border {
            width: 1,
            ..Border::default()
        };
        let widths: Vec<_> = AgentState::ALL.iter().map(|&s| b.width_for(s)).collect();
        assert_eq!(widths, vec![1, 2, 3, 4], "a thin border keeps four widths");
    }

    #[test]
    fn palettes_keep_four_distinct_colours_and_your_own_win() {
        for p in [Palette::Catppuccin, Palette::Colorblind] {
            let c = p.colours();
            let all = [c.working, c.waiting, c.done, c.failed];
            for (i, a) in all.iter().enumerate() {
                assert!(all[i + 1..].iter().all(|b| b != a), "{p:?}");
            }
        }
        let c = Config::from_json(r#"{"border":{"palette":"colorblind"}}"#).unwrap();
        assert_eq!(c.border.colours().waiting.to_hex(), "#e69f00");
        let own = Config::from_json(
            r##"{"border":{"palette":"colorblind","colours":{"waiting":"#ffffff"}}}"##,
        )
        .unwrap();
        assert_eq!(own.border.colours().waiting.to_hex(), "#ffffff");
        // The states left out keep the colour-blind palette.
        assert_eq!(own.border.colours().working.to_hex(), "#56b4e9");
        assert_eq!(own.border.colours().done.to_hex(), "#009e73");
        assert_eq!(own.border.colours().failed.to_hex(), "#d55e00");
        let text = serde_json::to_string(&own).unwrap();
        assert_eq!(Config::from_json(&text).unwrap(), own);
    }

    #[test]
    fn negative_gaps_and_absurd_borders_are_rejected() {
        assert!(Config::from_json(r#"{"gaps":{"outer":-1,"inner":0}}"#).is_err());
        assert!(Config::from_json(r#"{"border":{"width":100}}"#).is_err());
        assert!(Config::from_json(r#"{"border":{"offset":3000}}"#).is_err());
        assert!(Config::from_json(r#"{"border":{"offset":-65}}"#).is_err());
        assert!(Config::from_json(r#"{"border":{"offset":-8}}"#).is_ok());
        assert!(Config::from_json(r#"{"gaps":{"outer":2147483647,"inner":0}}"#).is_err());
    }

    #[test]
    fn size_limits_include_their_ends() {
        for text in [
            r#"{"border":{"offset":64}}"#,
            r#"{"border":{"offset":-64}}"#,
            r#"{"gaps":{"outer":1000,"inner":1000}}"#,
            r#"{"border":{"width":0}}"#,
            r#"{"border":{"width":1}}"#,
            r#"{"border":{"width":64}}"#,
        ] {
            assert!(Config::from_json(text).is_ok(), "{text}");
        }
        for text in [
            r#"{"border":{"offset":65}}"#,
            r#"{"border":{"offset":-65}}"#,
            r#"{"gaps":{"outer":1001,"inner":0}}"#,
            r#"{"gaps":{"outer":0,"inner":1001}}"#,
            r#"{"border":{"width":-1}}"#,
            r#"{"border":{"width":65}}"#,
        ] {
            assert!(
                matches!(Config::from_json(text), Err(ConfigError::Size(_))),
                "{text}"
            );
        }
    }

    #[test]
    fn a_fleet_named_only_spaces_has_no_name() {
        assert_eq!(
            Config::from_json(r#"{"fleets":[{"name":"  "}]}"#),
            Err(ConfigError::UnnamedFleet)
        );
    }

    #[test]
    fn the_default_config_serialises_and_reads_back_identical() {
        let text = serde_json::to_string(&Config::default()).unwrap();
        assert_eq!(Config::from_json(&text).unwrap(), Config::default());
    }

    #[test]
    fn a_fleet_signs_in_with_the_account_it_names() {
        let c = Config::from_json(
            r#"{"accounts":{"Work":{"claude":"C:\\Users\\me\\.claude-work"}},
                "fleets":[{"name":"crew","account":"work"},{"name":"home","hotkey":"ctrl + alt + h"}]}"#,
        )
        .unwrap();
        let work = c.account_of(&c.fleets[0]).unwrap();
        assert_eq!(
            work.variables(),
            vec![("CLAUDE_CONFIG_DIR", Path::new(r"C:\Users\me\.claude-work"))]
        );
        assert!(c.account_of(&c.fleets[1]).is_none());
        let text = serde_json::to_string(&c).unwrap();
        assert_eq!(Config::from_json(&text).unwrap(), c);
        let plain = serde_json::to_string(&Config::default()).unwrap();
        assert!(!plain.contains("account"), "{plain}");
    }

    #[test]
    fn an_account_names_both_folders_in_a_fixed_order() {
        let a = Account {
            codex: Some(PathBuf::from(r"D:\codex")),
            claude: Some(PathBuf::from(r"D:\claude")),
            warn: None,
            stop_at: Vec::new(),
        };
        let names: Vec<_> = a.variables().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["CLAUDE_CONFIG_DIR", "CODEX_HOME"]);
    }

    #[test]
    fn a_fleet_naming_a_missing_account_is_rejected() {
        assert_eq!(
            Config::from_json(r#"{"fleets":[{"name":"crew","account":"work"}]}"#),
            Err(ConfigError::UnknownAccount {
                name: "crew".into(),
                account: "work".into()
            })
        );
    }

    #[test]
    fn accounts_need_a_name_a_folder_and_full_paths() {
        for (text, name) in [
            (r#"{"accounts":{"work":{}}}"#, "work"),
            (r#"{"accounts":{" ":{"claude":"C:\\a"}}}"#, " "),
            (r#"{"accounts":{"work":{"claude":".claude-work"}}}"#, "work"),
            (r#"{"accounts":{"work":{"codex":"codex"}}}"#, "work"),
            (
                r#"{"accounts":{"Work":{"claude":"C:\\a"},"work":{"claude":"C:\\b"}}}"#,
                "work",
            ),
            (
                r#"{"accounts":{"a":{"claude":"C:\\x"},"b":{"claude":"c:\\X\\"}}}"#,
                "b",
            ),
        ] {
            assert!(
                matches!(Config::from_json(text), Err(ConfigError::Account { name: n, .. }) if n == name),
                "{text}"
            );
        }
        assert!(
            Config::from_json(r#"{"accounts":{"work":{"claude":"C:\\a","shell":"x"}}}"#).is_err()
        );
        for warn in ["0", "101"] {
            let text = format!(r#"{{"accounts":{{"work":{{"claude":"C:\\a","warn":{warn}}}}}}}"#);
            assert!(
                matches!(Config::from_json(&text), Err(ConfigError::Account { reason, .. }) if reason.contains("warn")),
                "{text}"
            );
        }
    }

    #[test]
    fn an_account_is_found_by_its_folder_in_any_case() {
        let c = Config::from_json(
            r#"{"accounts":{"work":{"claude":"C:\\Users\\Me\\.claude-work","warn":77},
                "codex-only":{"codex":"C:\\Users\\Me\\.claude"}}}"#,
        )
        .unwrap();
        let (name, work) = c
            .account_in(Path::new(r"c:\users\me\.CLAUDE-WORK\"))
            .unwrap();
        assert_eq!((name, work.warn), ("work", Some(77)));
        // Only Claude Code's folder counts: the status line is Claude Code's.
        assert!(c.account_in(Path::new(r"C:\Users\Me\.claude")).is_none());
    }

    #[test]
    fn the_schema_is_valid_json_naming_every_top_level_key() {
        let schema: serde_json::Value = serde_json::from_str(&Config::schema()).unwrap();
        for key in ["gaps", "border", "hotkeys", "fleets", "accounts"] {
            assert!(schema["properties"][key].is_object(), "{key}");
        }
    }

    fn rules(json: &str) -> Result<Config, ConfigError> {
        Config::from_json(&format!(
            r#"{{"accounts":{{"work":{{"claude":"C:\\w","stop_at":{json}}}}}}}"#
        ))
    }

    #[test]
    fn stop_rules_parse_and_bad_ones_name_their_problem() {
        let c = rules(
            r#"[{"share":90,"window":"five_hour","last_minutes":60},{"share":75,"days":"mon-fri","hours":"9-17"},{"share":95}]"#,
        )
        .unwrap();
        let stop = &c.account("work").unwrap().stop_at;
        assert_eq!(stop.len(), 3);
        assert_eq!(stop[0].window, Some(LimitWindow::FiveHour));
        for (json, says) in [
            (r#"[{"share":0}]"#, "1 to 100"),
            (r#"[{"share":101}]"#, "1 to 100"),
            (r#"[{"share":50,"days":"mon-fry"}]"#, "`fry`"),
            (r#"[{"share":50,"hours":"8"}]"#, "such as 8-18"),
            (r#"[{"share":50,"hours":"8-25"}]"#, "such as 8-18"),
            (r#"[{"share":50,"hours":"0-24"}]"#, "no time at all"),
            (r#"[{"share":50,"last_minutes":0}]"#, "a week"),
        ] {
            let e = rules(json).unwrap_err().to_string();
            assert!(e.contains(says), "{json}: {e}");
        }
        assert!(rules(r#"[{"share":50,"day":"mon"}]"#).is_err());
    }

    fn at(weekday: u8, hour: u16) -> LocalTime {
        LocalTime {
            now: 1_000_000,
            weekday,
            minute: hour * 60 + 30,
        }
    }

    #[test]
    fn a_rule_holds_on_its_days_in_its_hours_and_for_its_window() {
        let rule = |json: &str| serde_json::from_str::<StopAt>(json).unwrap();
        let work = rule(r#"{"share":75,"window":"five_hour","days":"mon-fri","hours":"9-17"}"#);
        let five = LimitWindow::FiveHour;
        let reset = 1_000_000 + 7200;
        assert!(work.holds(five, reset, at(0, 9)));
        assert!(!work.holds(five, reset, at(0, 8)));
        assert!(work.holds(five, reset, at(4, 16)));
        assert!(!work.holds(five, reset, at(4, 17)));
        assert!(!work.holds(five, reset, at(5, 10)));
        assert!(!work.holds(LimitWindow::SevenDay, reset, at(0, 10)));
        let night = rule(r#"{"share":50,"days":"sat,sun","hours":"22-6"}"#);
        assert!(night.holds(five, reset, at(6, 23)));
        assert!(night.holds(five, reset, at(5, 5)));
        assert!(!night.holds(five, reset, at(5, 6)));
        let round = rule(r#"{"share":50,"days":"fri-mon"}"#);
        assert!(round.holds(five, reset, at(6, 1)) && round.holds(five, reset, at(0, 1)));
        assert!(!round.holds(five, reset, at(1, 1)));
    }

    #[test]
    fn a_last_minutes_rule_holds_only_that_close_to_the_reset() {
        let rule: StopAt = serde_json::from_str(r#"{"share":90,"last_minutes":60}"#).unwrap();
        let w = LimitWindow::FiveHour;
        assert!(rule.holds(w, 1_000_000 + 3600, at(0, 9)));
        assert!(!rule.holds(w, 1_000_000 + 3601, at(0, 9)));
        assert!(!rule.holds(w, 1_000_000, at(0, 9)), "a reset window");
    }

    #[test]
    fn a_backlog_is_a_full_path() {
        let c =
            Config::from_json(r#"{"fleets":[{"name":"crew","backlog":"C:\\work\\BACKLOG.md"}]}"#)
                .unwrap();
        assert_eq!(
            c.fleets[0].backlog.as_deref(),
            Some(Path::new(r"C:\work\BACKLOG.md"))
        );
        assert!(matches!(
            Config::from_json(r#"{"fleets":[{"name":"crew","backlog":"BACKLOG.md"}]}"#),
            Err(ConfigError::Backlog(name)) if name == "crew"
        ));
    }
}
