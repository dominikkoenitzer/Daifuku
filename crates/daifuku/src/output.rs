//! What `--json` prints: one JSON document on standard output, described in
//! `docs/json-output.md`. Fields are added over time, never renamed or
//! removed within a major version.

#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;

/// Why a command failed, for a script to branch on. The message beside it
/// is for a person and may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The command line did not parse.
    Usage,
    /// The daemon is not running in this session.
    DaemonNotRunning,
    /// Windows refused the control pipe: the terminal is not elevated.
    NotElevated,
    /// The pipe to the daemon failed, or its answer did not read.
    Ipc,
    /// The daemon answered that the request did not work.
    Daemon,
    /// The config file could not be found or opened.
    Config,
    /// Daifuku runs on Windows only. Only a build for another system says so.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    /// Something that should not fail did.
    Internal,
}

/// `{"error": {"code": ..., "message": ...}}`, the one shape of every error.
#[derive(Debug, Serialize)]
pub struct ErrorDoc<'a> {
    pub error: ErrorBody<'a>,
}

/// The inside of [`ErrorDoc`].
#[derive(Debug, Serialize)]
pub struct ErrorBody<'a> {
    pub code: ErrorCode,
    pub message: &'a str,
}

impl<'a> ErrorDoc<'a> {
    pub const fn new(code: ErrorCode, message: &'a str) -> Self {
        Self {
            error: ErrorBody { code, message },
        }
    }
}

/// `{"ok": true}`: what a command that only does something prints.
#[derive(Debug, Serialize)]
pub struct Done {
    pub ok: bool,
}

/// What `daifuku config --path --json` prints.
#[derive(Debug, Serialize)]
pub struct ConfigPath {
    /// The config file, which need not exist.
    pub path: String,
}

/// One line of the doctor's report, as it is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A check, and what to do about it when it failed.
    Check { name: String, fix: Option<String> },
    /// A remark that is neither a pass nor a failure.
    Note(String),
    /// The folder the logs are in.
    Logs(String),
}

impl Finding {
    /// The line `daifuku doctor` prints for it.
    pub fn line(&self) -> String {
        match self {
            Self::Check { name, fix: None } => format!("ok    {name}"),
            Self::Check {
                name,
                fix: Some(fix),
            } => format!("FIX   {name}: {fix}"),
            Self::Note(text) => format!("--    {text}"),
            Self::Logs(dir) => format!("logs  {dir}"),
        }
    }
}

/// What `daifuku doctor --json` prints.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    /// This `daifuku`'s version.
    pub version: &'static str,
    /// Whether every check passed. The exit code says the same.
    pub ok: bool,
    /// Every check, in the order it ran.
    pub checks: Vec<DoctorCheck>,
    /// Remarks, such as checks skipped in a terminal that is not elevated.
    pub notes: Vec<String>,
    /// The folder the logs are in, or null.
    pub logs: Option<String>,
}

/// One check of [`DoctorReport`].
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    /// What to do about a failed check, null when it passed.
    pub fix: Option<String>,
}

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Fail,
}

impl DoctorReport {
    pub const fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            ok: true,
            checks: Vec::new(),
            notes: Vec::new(),
            logs: None,
        }
    }

    /// Takes in one finding.
    pub fn add(&mut self, finding: Finding) {
        match finding {
            Finding::Check { name, fix } => {
                let status = if fix.is_none() {
                    CheckStatus::Ok
                } else {
                    self.ok = false;
                    CheckStatus::Fail
                };
                self.checks.push(DoctorCheck { name, status, fix });
            }
            Finding::Note(text) => self.notes.push(text),
            Finding::Logs(dir) => self.logs = Some(dir),
        }
    }
}

/// `json` with every character past ASCII written as a `\u` escape. JSON has
/// such characters only inside strings, where the escape means the same, so
/// any parser reads the same value, in whatever code page it arrives.
pub fn ascii_json(json: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out
}

/// `value` as one line of ASCII JSON.
pub fn to_json<T: Serialize>(value: &T) -> String {
    // These types serialise without fail; the fallback keeps stdout JSON.
    let json = serde_json::to_string(value).unwrap_or_else(|_| {
        "{\"error\":{\"code\":\"internal\",\"message\":\"could not write JSON\"}}".to_owned()
    });
    ascii_json(&json)
}

/// Prints `value` as the one JSON document on standard output.
pub fn print_json<T: Serialize>(value: &T) {
    println!("{}", to_json(value));
}
