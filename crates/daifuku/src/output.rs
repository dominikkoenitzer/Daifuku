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
    /// The config file is not one the daemon would load.
    InvalidConfig,
    /// Daifuku runs on Windows only. Only a build for another system says so.
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

/// What `daifuku config validate --json` prints for a file that passed.
#[derive(Debug, Serialize)]
pub struct Validated {
    pub ok: bool,
    /// The file that was checked.
    pub path: String,
}

/// What `daifuku config --path --json` prints.
#[derive(Debug, Serialize)]
pub struct ConfigPath {
    /// The config file, which need not exist.
    pub path: String,
}

/// What `install --dry-run` and `uninstall --dry-run` print: the steps the
/// real command would take, worked out by the same code.
#[derive(Debug, Serialize)]
pub struct Plan {
    /// Always true: nothing was changed.
    pub dry_run: bool,
    /// `install` or `uninstall`.
    pub command: &'static str,
    /// Whether this terminal is elevated, as the real command needs.
    pub elevated: bool,
    /// Every change, in the order the real command makes it.
    pub steps: Vec<PlanStep>,
    /// Remarks that are not steps.
    pub notes: Vec<String>,
}

/// One change of a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanStep {
    /// What happens, such as `copy` or `register_task`.
    pub action: &'static str,
    /// The file, folder or task it happens to.
    pub target: String,
    /// More about it, or null.
    pub detail: Option<String>,
}

impl Plan {
    pub const fn new(command: &'static str, elevated: bool) -> Self {
        Self {
            dry_run: true,
            command,
            elevated,
            steps: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The lines a dry run prints without `--json`.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "dry run      nothing changes; `daifuku {}` would do this:",
            self.command
        )];
        for s in &self.steps {
            let label = format!("would {}", s.action.replace('_', " "));
            out.push(match &s.detail {
                Some(d) => format!("{label:<22} {} ({d})", s.target),
                None => format!("{label:<22} {}", s.target),
            });
        }
        for n in &self.notes {
            out.push(format!("--    {n}"));
        }
        out
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use daifuku_core::protocol::{AgentWindow, FleetStatus, Response, Status, to_line};
    use daifuku_core::state::AgentState;
    use serde_json::json;

    fn value<T: Serialize>(v: &T) -> serde_json::Value {
        serde_json::from_str(&to_json(v)).unwrap()
    }

    #[test]
    fn an_error_has_one_shape() {
        let doc = ErrorDoc::new(ErrorCode::DaemonNotRunning, "Daifuku is not running");
        assert_eq!(
            to_json(&doc),
            r#"{"error":{"code":"daemon_not_running","message":"Daifuku is not running"}}"#
        );
    }

    #[test]
    fn every_error_code_is_snake_case() {
        let codes = [
            (ErrorCode::Usage, "usage"),
            (ErrorCode::DaemonNotRunning, "daemon_not_running"),
            (ErrorCode::NotElevated, "not_elevated"),
            (ErrorCode::Ipc, "ipc"),
            (ErrorCode::Daemon, "daemon"),
            (ErrorCode::Config, "config"),
            (ErrorCode::InvalidConfig, "invalid_config"),
            (ErrorCode::Unsupported, "unsupported"),
            (ErrorCode::Internal, "internal"),
        ];
        for (code, name) in codes {
            assert_eq!(value(&code), json!(name));
        }
    }

    #[test]
    fn a_done_command_says_ok() {
        assert_eq!(to_json(&Done { ok: true }), r#"{"ok":true}"#);
    }

    #[test]
    fn a_valid_config_says_ok_and_which_file() {
        let doc = Validated {
            ok: true,
            path: "C:\\ProgramData\\Daifuku\\daifuku.json".into(),
        };
        assert_eq!(
            to_json(&doc),
            r#"{"ok":true,"path":"C:\\ProgramData\\Daifuku\\daifuku.json"}"#
        );
    }

    #[test]
    fn the_config_path_is_one_field() {
        let path = ConfigPath {
            path: "C:\\ProgramData\\Daifuku\\daifuku.json".into(),
        };
        assert_eq!(
            to_json(&path),
            r#"{"path":"C:\\ProgramData\\Daifuku\\daifuku.json"}"#
        );
    }

    #[test]
    fn the_doctor_report_lists_each_check_with_its_fix() {
        let mut report = DoctorReport::new();
        report.add(Finding::Check {
            name: "installed in Program Files".into(),
            fix: None,
        });
        report.add(Finding::Note("daemon busy".into()));
        report.add(Finding::Check {
            name: "daemon running".into(),
            fix: Some("sign out and in".into()),
        });
        report.add(Finding::Logs("C:\\logs".into()));
        assert!(!report.ok);
        assert_eq!(
            value(&report),
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "ok": false,
                "checks": [
                    {"name": "installed in Program Files", "status": "ok", "fix": null},
                    {"name": "daemon running", "status": "fail", "fix": "sign out and in"},
                ],
                "notes": ["daemon busy"],
                "logs": "C:\\logs",
            })
        );
    }

    #[test]
    fn a_doctor_report_with_no_failure_is_ok() {
        let mut report = DoctorReport::new();
        report.add(Finding::Check {
            name: "config valid".into(),
            fix: None,
        });
        assert!(report.ok);
        assert_eq!(value(&report)["ok"], json!(true));
        assert_eq!(value(&report)["logs"], json!(null));
    }

    #[test]
    fn doctor_lines_read_as_they_always_have() {
        let check = |fix: Option<&str>| Finding::Check {
            name: "config valid".into(),
            fix: fix.map(str::to_owned),
        };
        assert_eq!(check(None).line(), "ok    config valid");
        assert_eq!(
            check(Some("no ProgramData folder")).line(),
            "FIX   config valid: no ProgramData folder"
        );
        assert_eq!(
            Finding::Note("elevation and hotkeys".into()).line(),
            "--    elevation and hotkeys"
        );
        assert_eq!(Finding::Logs("C:\\logs".into()).line(), "logs  C:\\logs");
    }

    #[test]
    fn json_past_ascii_is_escaped() {
        let doc = ErrorDoc::new(ErrorCode::Daemon, "no fleet Gr\u{fc}ezi");
        assert_eq!(
            to_json(&doc),
            r#"{"error":{"code":"daemon","message":"no fleet Gr\u00fcezi"}}"#
        );
    }

    #[test]
    fn status_json_keeps_the_daemon_reply_shape() {
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
        let line = to_line(&Response::Status(status)).unwrap();
        let read: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            read,
            json!({
                "result": "status",
                "version": "0.1.0",
                "config": "C:\\ProgramData\\Daifuku\\daifuku.json",
                "elevated": true,
                "hotkeys": ["ctrl + alt + return: open agents"],
                "fleets": [{"name": "agents", "monitor": "portrait", "windows": [1, 2]}],
                "agents": [{"window": 1, "title": "claude", "state": "waiting", "for_seconds": 73}],
            })
        );
    }

    #[test]
    fn status_json_from_an_older_daemon_still_has_every_field() {
        let old = r#"{"result":"status","version":"0.0.9","config":"c","fleets":[],"agents":[]}"#;
        let Ok(Response::Status(status)) = daifuku_core::protocol::from_line::<Response>(old)
        else {
            panic!("an older reply did not read");
        };
        let line = to_line(&Response::Status(status)).unwrap();
        assert_eq!(
            line,
            "{\"result\":\"status\",\"version\":\"0.0.9\",\"config\":\"c\",\"elevated\":false,\"hotkeys\":[],\"fleets\":[],\"agents\":[]}\n"
        );
    }
}
