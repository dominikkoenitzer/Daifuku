//! The config file, `C:\ProgramData\Daifuku\daifuku.json`.
//!
//! Every key is optional. An empty file, or no file, is a working setup: one
//! fleet of six admin terminals running `claude` on the monitor turned on its
//! side, or on the primary one when none is.
//!
//! ```json
//! {
//!   "$schema": "https://raw.githubusercontent.com/dominikkoenitzer/Daifuku/main/schema.json",
//!   "fleets": [
//!     { "name": "agents", "count": 6, "monitor": "portrait", "hotkey": "ctrl + alt + return" },
//!     { "name": "site", "count": 4, "directory": "C:\\src\\site", "hotkey": "ctrl + alt + w" }
//!   ]
//! }
//! ```

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::grid::{Gaps, Shape};
use crate::hotkey::Hotkey;
use crate::state::AgentState;

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
    /// Play the system notification sound when an agent starts waiting for
    /// you, for when you are not looking at the screen.
    pub sound: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: None,
            gaps: Gaps::default(),
            border: Border::default(),
            hotkeys: Hotkeys::default(),
            fleets: vec![Fleet::default()],
            sound: false,
        }
    }
}

/// The status border.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Border {
    /// Draw status borders at all.
    pub enabled: bool,
    /// Thickness in physical pixels.
    pub width: i32,
    /// How far outside the visible frame the border sits; negative overlaps
    /// the window's own edge.
    pub offset: i32,
    /// The colours, as a set. `catppuccin` by default; `colorblind` is the
    /// Okabe-Ito palette, which stays distinct for every common kind of
    /// colour blindness.
    pub palette: Palette,
    /// Colours of your own, one per state, instead of the palette's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colours: Option<StateColours>,
    /// Draw each state at its own width, so a state reads without its
    /// colour: at a width of 4, done is 2, working 4, failed 6 and waiting 8.
    pub state_widths: bool,
    /// Let a waiting border breathe slowly, the one thing on screen that
    /// moves. Off whenever Windows is set to show no animations.
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
    /// The colours in use: your own, else the palette's.
    #[must_use]
    pub fn colours(&self) -> StateColours {
        self.colours.unwrap_or_else(|| self.palette.colours())
    }

    /// The thickness a state is drawn at.
    #[must_use]
    pub fn width_for(&self, state: AgentState) -> i32 {
        let w = self.width.max(1);
        if !self.state_widths {
            return w;
        }
        match state {
            AgentState::Done => (w / 2).max(1),
            AgentState::Working => w,
            AgentState::Failed => w + w / 2,
            AgentState::Waiting => w * 2,
        }
    }
}

/// A named set of state colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    /// Catppuccin Mocha: blue, yellow, green and red.
    #[default]
    Catppuccin,
    /// Okabe-Ito: sky blue, orange, bluish green and vermilion, chosen to stay
    /// distinct for deuteranopia, protanopia and tritanopia.
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
            },
            Self::Colorblind => StateColours {
                working: Colour::new(0x56, 0xb4, 0xe9),
                waiting: Colour::new(0xe6, 0x9f, 0x00),
                done: Colour::new(0x00, 0x9e, 0x73),
                failed: Colour::new(0xd5, 0x5e, 0x00),
            },
        }
    }
}

/// A colour for each agent state. The defaults are Catppuccin Mocha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StateColours {
    /// Thinking and calling tools. Blue.
    pub working: Colour,
    /// Blocked on you. Yellow, the one to look for.
    pub waiting: Colour,
    /// Finished its turn. Green.
    pub done: Colour,
    /// The turn ended on an error. Red.
    pub failed: Colour,
}

impl Default for StateColours {
    fn default() -> Self {
        Self {
            working: Colour::new(0x89, 0xb4, 0xfa),
            waiting: Colour::new(0xf9, 0xe2, 0xaf),
            done: Colour::new(0xa6, 0xe3, 0xa1),
            failed: Colour::new(0xf3, 0x8b, 0xa8),
        }
    }
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
}

impl Default for Hotkeys {
    /// Ctrl and Alt, because common tiling setups live on Alt alone, and keys
    /// that type nothing with AltGr on the German, Swiss and French layouts,
    /// where AltGr is Ctrl and Alt together. Not Ctrl, Alt and Space: on the
    /// first machine Daifuku ran on, another program already held it.
    fn default() -> Self {
        Self {
            next_waiting: Some(HotkeyText::new("ctrl + alt + n")),
            snap: Some(HotkeyText::new("ctrl + alt + backspace")),
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
    /// Open the terminals as administrator.
    pub admin: bool,
    /// Start PowerShell without the user's profile script.
    pub no_profile: bool,
    /// The key that opens this fleet, or brings it back if it is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<HotkeyText>,
}

impl Default for Fleet {
    fn default() -> Self {
        Self {
            name: "agents".to_owned(),
            count: 6,
            monitor: MonitorPick::Portrait,
            shape: None,
            directory: None,
            command: Some("claude".to_owned()),
            profile: None,
            admin: true,
            no_profile: false,
            hotkey: Some(HotkeyText::new("ctrl + alt + return")),
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
        schemars::json_schema!({
            "description": "portrait, landscape, primary, secondary, cursor, or a device name such as \\\\.\\DISPLAY2",
            "anyOf": [
                { "enum": ["portrait", "landscape", "primary", "secondary", "cursor"] },
                { "type": "string" }
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
    /// Gaps or border sizes that make no sense.
    #[error("{0}")]
    Size(String),
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
            serde_json::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?
        };
        config.validate()?;
        Ok(config)
    }

    /// Checks everything a type cannot: unique names, counts in range, every
    /// hotkey parses and none is bound twice.
    ///
    /// # Errors
    ///
    /// The first problem found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.gaps.outer < 0 || self.gaps.inner < 0 {
            return Err(ConfigError::Size("gaps cannot be negative".into()));
        }
        if !(0..=64).contains(&self.border.width) {
            return Err(ConfigError::Size(
                "border width must be 0 to 64 pixels".into(),
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

    fn all_hotkeys(&self) -> impl Iterator<Item = &HotkeyText> {
        self.hotkeys
            .next_waiting
            .iter()
            .chain(self.hotkeys.snap.iter())
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
        assert_eq!(defs["Colour"]["pattern"], "^#?[0-9a-fA-F]{6}$");
    }

    #[test]
    fn zero_gaps_are_allowed() {
        assert!(Config::from_json(r#"{"gaps":{"outer":0,"inner":0}}"#).is_ok());
        assert!(Config::from_json(r#"{"gaps":{"outer":0,"inner":-1}}"#).is_err());
    }

    #[test]
    fn no_file_is_one_fleet_of_six_admin_claudes_on_the_portrait_monitor() {
        let c = Config::from_json("").unwrap();
        assert_eq!(c.fleets.len(), 1);
        let f = &c.fleets[0];
        assert_eq!(
            (f.count, f.admin, f.command.as_deref()),
            (6, true, Some("claude"))
        );
        assert_eq!(f.monitor, MonitorPick::Portrait);
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
          "$schema": "https://raw.githubusercontent.com/dominikkoenitzer/Daifuku/main/schema.json",
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
        let thin = Border {
            width: 1,
            ..Border::default()
        };
        assert!(AgentState::ALL.iter().all(|&s| thin.width_for(s) >= 1));
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
    }

    #[test]
    fn negative_gaps_and_absurd_borders_are_rejected() {
        assert!(Config::from_json(r#"{"gaps":{"outer":-1,"inner":0}}"#).is_err());
        assert!(Config::from_json(r#"{"border":{"width":100}}"#).is_err());
    }

    #[test]
    fn the_default_config_serialises_and_reads_back_identical() {
        let text = serde_json::to_string(&Config::default()).unwrap();
        assert_eq!(Config::from_json(&text).unwrap(), Config::default());
    }

    #[test]
    fn the_schema_is_valid_json_naming_every_top_level_key() {
        let schema: serde_json::Value = serde_json::from_str(&Config::schema()).unwrap();
        for key in ["gaps", "border", "hotkeys", "fleets"] {
            assert!(schema["properties"][key].is_object(), "{key}");
        }
    }
}
