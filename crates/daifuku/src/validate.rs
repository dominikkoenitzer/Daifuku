//! `daifuku config validate`: whether a config file would load, checked
//! without the daemon.
//!
//! It reads the file with the same two calls the daemon and `daifuku doctor`
//! use, [`config::decode`] and [`Config::from_json`], so a file passes here
//! exactly when the daemon would take it. `schema.json` is generated from the
//! same types, so the parser is the schema check.

use std::path::Path;

use daifuku_core::config::{self, Config};

use crate::output::ErrorCode;

/// Why a file did not validate: the code a script branches on, and the
/// message for a person.
#[derive(Debug, PartialEq, Eq)]
pub struct Problem {
    pub code: ErrorCode,
    pub message: String,
}

/// Checks the config file at `path`.
///
/// # Errors
///
/// `config` when the file is missing or cannot be read, `invalid_config`
/// when it is not text or not a config the daemon would load.
pub fn file(path: &Path) -> Result<(), Problem> {
    let bytes = std::fs::read(path).map_err(|e| Problem {
        code: ErrorCode::Config,
        message: if e.kind() == std::io::ErrorKind::NotFound {
            format!("there is no config at {}", path.display())
        } else {
            format!("could not read {}: {e}", path.display())
        },
    })?;
    bytes_of(&bytes)
}

/// Checks a config file's contents.
///
/// # Errors
///
/// `invalid_config` when the bytes are not UTF-8 or UTF-16 text, or not a
/// config the daemon would load.
pub fn bytes_of(bytes: &[u8]) -> Result<(), Problem> {
    let invalid = |message: String| Problem {
        code: ErrorCode::InvalidConfig,
        message,
    };
    let text = config::decode(bytes)
        .map_err(|e| invalid(format!("the file is neither UTF-8 nor UTF-16 text: {e}")))?;
    Config::from_json(&text).map_err(|e| invalid(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starter() -> String {
        let c = Config {
            schema: Some(format!("{}/schema.json", config::SITE)),
            ..Config::default()
        };
        serde_json::to_string_pretty(&c).unwrap()
    }

    fn problem(text: &str) -> Problem {
        bytes_of(text.as_bytes()).unwrap_err()
    }

    #[test]
    fn the_starter_config_and_an_empty_file_pass() {
        assert_eq!(bytes_of(starter().as_bytes()), Ok(()));
        assert_eq!(bytes_of(b""), Ok(()));
        assert_eq!(bytes_of(b"{}"), Ok(()));
    }

    #[test]
    fn an_unknown_key_fails_with_where_it_is() {
        let p = problem("{\n  \"fleets\": [{ \"name\": \"x\", \"cuont\": 3 }]\n}");
        assert_eq!(p.code, ErrorCode::InvalidConfig);
        assert!(p.message.contains("cuont"), "{}", p.message);
        assert!(p.message.contains("line 2 column"), "{}", p.message);
    }

    #[test]
    fn a_hotkey_that_does_not_parse_fails() {
        let p = problem(r#"{"hotkeys":{"snap":"ctrl + banana"}}"#);
        assert_eq!(p.code, ErrorCode::InvalidConfig);
        assert!(p.message.contains("ctrl + banana"), "{}", p.message);
    }

    #[test]
    fn a_colour_that_is_not_rrggbb_fails() {
        let p = problem(r##"{"border":{"colours":{"waiting":"#ffff"}}}"##);
        assert_eq!(p.code, ErrorCode::InvalidConfig);
        assert!(p.message.contains("#ffff"), "{}", p.message);
    }

    #[test]
    fn two_fleets_with_one_name_fail() {
        let p = problem(r#"{"fleets":[{"name":"a","hotkey":null},{"name":"A","hotkey":null}]}"#);
        assert_eq!(p.code, ErrorCode::InvalidConfig);
        assert!(p.message.contains("two fleets"), "{}", p.message);
    }

    #[test]
    fn a_misspelt_monitor_fails() {
        let p = problem(r#"{"fleets":[{"name":"a","monitor":"portait"}]}"#);
        assert_eq!(p.code, ErrorCode::InvalidConfig);
    }

    #[test]
    fn a_file_with_a_byte_order_mark_passes_as_the_daemon_reads_it() {
        let mut utf8 = vec![0xEF, 0xBB, 0xBF];
        utf8.extend_from_slice(starter().as_bytes());
        assert_eq!(bytes_of(&utf8), Ok(()));
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(starter().encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(bytes_of(&utf16), Ok(()));
    }

    #[test]
    fn bytes_that_are_not_text_fail() {
        let p = bytes_of(&[0xC3, 0x28]).unwrap_err();
        assert_eq!(p.code, ErrorCode::InvalidConfig);
    }

    #[test]
    fn a_missing_file_is_a_config_error_not_an_invalid_one() {
        let dir = std::env::temp_dir().join(format!("daifuku-validate-{}", std::process::id()));
        let p = file(&dir.join("daifuku.json")).unwrap_err();
        assert_eq!(p.code, ErrorCode::Config);
        assert!(
            p.message.starts_with("there is no config at"),
            "{}",
            p.message
        );
    }
}
