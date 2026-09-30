//! Hotkeys as the config file writes them: `ctrl + alt + return`.
//!
//! Parsing is pure. The daemon hands the result to `RegisterHotKey`, which is
//! why a [`Hotkey`] is stored as exactly the two numbers that call takes.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// `MOD_ALT` as `RegisterHotKey` spells it.
pub const MOD_ALT: u32 = 0x0001;
/// `MOD_CONTROL`.
pub const MOD_CONTROL: u32 = 0x0002;
/// `MOD_SHIFT`.
pub const MOD_SHIFT: u32 = 0x0004;
/// `MOD_WIN`.
pub const MOD_WIN: u32 = 0x0008;

/// A key combination: modifiers plus one virtual key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hotkey {
    /// `MOD_*` flags.
    pub modifiers: u32,
    /// The virtual-key code.
    pub vk: u16,
}

/// Why a hotkey did not parse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HotkeyError {
    /// Nothing but whitespace and plus signs.
    #[error("the hotkey is empty")]
    Empty,
    /// A name that is neither a modifier nor a key Daifuku knows.
    #[error("`{0}` is not a key name")]
    UnknownKey(String),
    /// Two non-modifier keys, `a + b`.
    #[error("a hotkey has one key besides its modifiers, `{0}` is a second one")]
    SecondKey(String),
    /// Modifiers only.
    #[error("the hotkey has modifiers but no key")]
    NoKey,
    /// No modifier at all: a bare key would be taken from every program.
    #[error("a hotkey needs at least one of ctrl, alt, shift or win")]
    NoModifier,
}

impl FromStr for Hotkey {
    type Err = HotkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut modifiers = 0;
        let mut vk = None;
        let mut any = false;
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            any = true;
            let lower = part.to_ascii_lowercase();
            if let Some(m) = modifier(&lower) {
                modifiers |= m;
                continue;
            }
            let code = key(&lower).ok_or_else(|| HotkeyError::UnknownKey(part.to_owned()))?;
            if vk.replace(code).is_some() {
                return Err(HotkeyError::SecondKey(part.to_owned()));
            }
        }
        if !any {
            return Err(HotkeyError::Empty);
        }
        let vk = vk.ok_or(HotkeyError::NoKey)?;
        if modifiers == 0 {
            return Err(HotkeyError::NoModifier);
        }
        Ok(Self { modifiers, vk })
    }
}

impl fmt::Display for Hotkey {
    /// The canonical spelling, modifiers in a fixed order, so two ways of
    /// writing one combination print the same.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (MOD_CONTROL, "ctrl"),
            (MOD_ALT, "alt"),
            (MOD_SHIFT, "shift"),
            (MOD_WIN, "win"),
        ] {
            if self.modifiers & flag != 0 {
                write!(f, "{name} + ")?;
            }
        }
        f.write_str(&key_name(self.vk))
    }
}

fn modifier(name: &str) -> Option<u32> {
    Some(match name {
        "ctrl" | "control" => MOD_CONTROL,
        "alt" => MOD_ALT,
        "shift" => MOD_SHIFT,
        "win" | "super" | "windows" => MOD_WIN,
        _ => return None,
    })
}

/// Named keys that are not a single letter, digit or function key.
const NAMED: &[(&str, u16)] = &[
    ("backspace", 0x08),
    ("tab", 0x09),
    ("return", 0x0D),
    ("enter", 0x0D),
    ("pause", 0x13),
    ("escape", 0x1B),
    ("esc", 0x1B),
    ("space", 0x20),
    ("pageup", 0x21),
    ("pagedown", 0x22),
    ("end", 0x23),
    ("home", 0x24),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("insert", 0x2D),
    ("delete", 0x2E),
    ("semicolon", 0xBA),
    ("plus", 0xBB),
    ("equal", 0xBB),
    ("comma", 0xBC),
    ("minus", 0xBD),
    ("period", 0xBE),
    ("slash", 0xBF),
    ("grave", 0xC0),
    ("bracketleft", 0xDB),
    ("backslash", 0xDC),
    ("bracketright", 0xDD),
    ("quote", 0xDE),
];

fn key(name: &str) -> Option<u16> {
    if let Some(&(_, vk)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(vk);
    }
    let bytes = name.as_bytes();
    if bytes.len() == 1 && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return Some(u16::from(bytes[0].to_ascii_uppercase()));
    }
    if let Some(n) = name
        .strip_prefix("numpad")
        .and_then(|d| d.parse::<u16>().ok())
        && n <= 9
    {
        return Some(0x60 + n);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|d| d.parse::<u16>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x70 + n - 1);
    }
    None
}

fn key_name(vk: u16) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(u8::try_from(vk).unwrap_or(b'?'))
            .to_ascii_lowercase()
            .to_string(),
        0x60..=0x69 => format!("numpad{}", vk - 0x60),
        0x70..=0x87 => format!("f{}", vk - 0x70 + 1),
        _ => NAMED
            .iter()
            .find(|(_, code)| *code == vk)
            .map_or_else(|| format!("vk{vk:#04x}"), |(n, _)| (*n).to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_print_as_themselves() {
        assert_eq!(hk("alt + q").to_string(), "alt + q");
        assert_eq!(hk("ctrl + 7").to_string(), "ctrl + 7");
    }

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    #[test]
    fn parses_the_default_fleet_key() {
        assert_eq!(
            hk("ctrl + alt + return"),
            Hotkey {
                modifiers: MOD_CONTROL | MOD_ALT,
                vk: 0x0D
            }
        );
    }

    #[test]
    fn spacing_case_and_order_do_not_matter() {
        assert_eq!(hk("Alt+CTRL+Return"), hk("ctrl + alt + return"));
        assert_eq!(hk("enter + ctrl"), hk("ctrl + return"));
    }

    #[test]
    fn letters_digits_function_and_numpad_keys() {
        assert_eq!(hk("alt + q").vk, u16::from(b'Q'));
        assert_eq!(hk("alt + 7").vk, u16::from(b'7'));
        assert_eq!(hk("alt + f1").vk, 0x70);
        assert_eq!(hk("alt + f24").vk, 0x87);
        assert_eq!(hk("alt + numpad0").vk, 0x60);
    }

    #[test]
    fn rejects_what_would_misbehave() {
        assert_eq!("".parse::<Hotkey>(), Err(HotkeyError::Empty));
        assert_eq!(" + ".parse::<Hotkey>(), Err(HotkeyError::Empty));
        assert_eq!("ctrl + alt".parse::<Hotkey>(), Err(HotkeyError::NoKey));
        assert_eq!("return".parse::<Hotkey>(), Err(HotkeyError::NoModifier));
        assert_eq!(
            "ctrl + a + b".parse::<Hotkey>(),
            Err(HotkeyError::SecondKey("b".into()))
        );
        assert_eq!(
            "ctrl + f25".parse::<Hotkey>(),
            Err(HotkeyError::UnknownKey("f25".into()))
        );
        assert_eq!(
            "ctrl + numpad10".parse::<Hotkey>(),
            Err(HotkeyError::UnknownKey("numpad10".into()))
        );
        assert_eq!(
            "ctrl + banana".parse::<Hotkey>(),
            Err(HotkeyError::UnknownKey("banana".into()))
        );
    }

    #[test]
    fn display_is_canonical_and_parses_back() {
        for s in [
            "alt + ctrl + enter",
            "shift+win+f5",
            "ctrl + alt + space",
            "ctrl + numpad3",
            "alt + grave",
        ] {
            let parsed = hk(s);
            assert_eq!(hk(&parsed.to_string()), parsed, "{s}");
        }
        assert_eq!(hk("alt + ctrl + enter").to_string(), "ctrl + alt + return");
    }
}
