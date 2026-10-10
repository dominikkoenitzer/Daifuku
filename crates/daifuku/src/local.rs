//! What `daifuku status` says from a terminal that is not elevated.
//!
//! The daemon answers only elevated processes, as a request on its control
//! pipe can open administrator terminals, and that stays so. From any other
//! terminal `status` leaves the pipe alone and reports what needs no daemon:
//! whether Daifuku is installed and which version, whether the daemon process
//! runs, whether the logon task is there, and where the config is and
//! whether it loads. Fleets, agents and hotkeys need an administrator
//! terminal, and the report says so in a field of its own.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{output, validate};

/// The daemon's file name, as the process list shows it.
pub const DAEMON: &str = "daifukud.exe";

/// What `live_state` says: fleets, agents and hotkeys need an administrator
/// terminal.
pub const NEEDS_ADMINISTRATOR: &str = "needs_administrator_terminal";

/// Where the facts come from: Windows in the program, fakes in the tests.
/// Nothing here may open the daemon's pipes.
pub trait Probe {
    /// Whether both programs are in the install folder.
    fn installed(&self) -> bool;
    /// The version installed, when it can be told.
    fn installed_version(&self) -> Option<String>;
    /// Whether a daemon process runs in this session.
    fn daemon_running(&self) -> bool;
    /// Whether this user's logon task is registered.
    fn logon_task(&self) -> bool;
    /// The config file, `None` without a ProgramData folder.
    fn config_file(&self) -> Option<PathBuf>;
    /// The config file's contents.
    fn read(&self, file: &Path) -> std::io::Result<Vec<u8>>;
}

/// What `daifuku status --json` prints from a terminal that is not elevated.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct LocalStatus {
    /// Always `local_status`, where the daemon's reply says `status`.
    pub result: &'static str,
    /// This `daifuku`'s version.
    pub version: &'static str,
    /// Whether both programs are in Program Files.
    pub installed: bool,
    /// The installed version, null when not installed or not known.
    pub installed_version: Option<String>,
    /// Whether a daemon process runs in this session.
    pub daemon_running: bool,
    /// Whether this user's logon task is registered, or the one task
    /// earlier installs shared runs for this user.
    pub logon_task: bool,
    /// The config file, which need not exist; null without a ProgramData
    /// folder.
    pub config: Option<String>,
    /// Whether the config file exists. Without one the defaults apply.
    pub config_exists: bool,
    /// Whether the daemon would load the config: true for no file too.
    pub config_valid: bool,
    /// What is wrong with the config, null when it is valid.
    pub config_error: Option<String>,
    /// Always [`NEEDS_ADMINISTRATOR`]: fleets, agents and hotkeys come only
    /// from the daemon, which answers only an administrator terminal.
    pub live_state: &'static str,
    /// The same for a person, ending in the command to run there.
    pub message: String,
}

/// Gathers the report. `command` is the command to run again in an
/// administrator terminal.
pub fn gather(probe: &impl Probe, command: &str) -> LocalStatus {
    let installed = probe.installed();
    let file = probe.config_file();
    let (config_exists, config_error) = match &file {
        None => (false, Some("no ProgramData folder".to_owned())),
        Some(f) => match probe.read(f) {
            Ok(bytes) => (
                true,
                validate::bytes_of(&bytes)
                    .err()
                    .map(|p| one_line(&p.message)),
            ),
            // No file is the default config, as the daemon takes it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, None),
            Err(e) => (true, Some(one_line(&format!("could not read it: {e}")))),
        },
    };
    LocalStatus {
        result: "local_status",
        version: env!("CARGO_PKG_VERSION"),
        installed,
        installed_version: installed.then(|| probe.installed_version()).flatten(),
        daemon_running: probe.daemon_running(),
        logon_task: probe.logon_task(),
        config: file.map(|f| f.display().to_string()),
        config_exists,
        config_valid: config_error.is_none(),
        config_error,
        live_state: NEEDS_ADMINISTRATOR,
        message: format!(
            "fleets, agents and hotkeys need an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: {command}"
        ),
    }
}

impl LocalStatus {
    /// The one line `daifuku status` prints without `--json`.
    pub fn line(&self) -> String {
        let installed = match (&self.installed_version, self.installed) {
            (Some(v), true) => format!("daifuku {v} installed"),
            (None, true) => "daifuku installed".to_owned(),
            (_, false) => "daifuku not installed".to_owned(),
        };
        let daemon = if self.daemon_running {
            "daemon running"
        } else {
            "daemon not running"
        };
        let task = if self.logon_task {
            "logon task registered"
        } else {
            "no logon task"
        };
        let config = match (&self.config, &self.config_error) {
            (None, _) => "no ProgramData folder".to_owned(),
            (Some(path), Some(e)) => format!("config invalid ({path}): {e}"),
            (Some(path), None) if self.config_exists => format!("config valid ({path})"),
            (Some(path), None) => format!("no config, the defaults apply ({path})"),
        };
        format!("{installed}, {daemon}, {task}, {config}; {}", self.message)
    }
}

/// What `status` prints from a terminal that is not elevated, and its exit
/// code. The code is 0 whatever the report says: it is a report, not a
/// failure, and the fields say what is missing.
pub fn report(status: &LocalStatus, json: bool) -> (String, u8) {
    let text = if json {
        output::to_json(status)
    } else {
        status.line()
    };
    (text, 0)
}

/// Whether a daemon runs among `processes`, each an id and an executable
/// name, counting only those `in_session` says are in this session: another
/// person signed in at the same time has a daemon of their own.
pub fn daemon_in<'a>(
    processes: impl IntoIterator<Item = (u32, &'a str)>,
    in_session: impl Fn(u32) -> bool,
) -> bool {
    processes
        .into_iter()
        .any(|(pid, name)| name.eq_ignore_ascii_case(DAEMON) && in_session(pid))
}

/// `text` on one line, so the report stays one line.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FILE: &str = "C:\\ProgramData\\Daifuku\\daifuku.json";
    const COMMAND: &str = "daifuku status --json";

    /// A PC as the test describes it. Nothing here asks Windows.
    struct Fake {
        installed: bool,
        version: Option<&'static str>,
        daemon: bool,
        task: bool,
        file: Option<&'static str>,
        config: Result<&'static str, std::io::ErrorKind>,
    }

    impl Fake {
        /// Installed, running and registered, with a valid config.
        const fn all_there() -> Self {
            Self {
                installed: true,
                version: Some("0.1.2"),
                daemon: true,
                task: true,
                file: Some(FILE),
                config: Ok("{}"),
            }
        }
    }

    impl Probe for Fake {
        fn installed(&self) -> bool {
            self.installed
        }
        fn installed_version(&self) -> Option<String> {
            self.version.map(str::to_owned)
        }
        fn daemon_running(&self) -> bool {
            self.daemon
        }
        fn logon_task(&self) -> bool {
            self.task
        }
        fn config_file(&self) -> Option<PathBuf> {
            self.file.map(PathBuf::from)
        }
        fn read(&self, file: &Path) -> std::io::Result<Vec<u8>> {
            assert_eq!(Some(file), self.file.map(Path::new));
            self.config
                .map(|text| text.as_bytes().to_vec())
                .map_err(std::io::Error::from)
        }
    }

    fn value(status: &LocalStatus) -> serde_json::Value {
        serde_json::from_str(&report(status, true).0).unwrap()
    }

    #[test]
    fn a_working_setup_reports_every_field_and_points_to_an_administrator_terminal() {
        let status = gather(&Fake::all_there(), COMMAND);
        assert_eq!(
            value(&status),
            json!({
                "result": "local_status",
                "version": env!("CARGO_PKG_VERSION"),
                "installed": true,
                "installed_version": "0.1.2",
                "daemon_running": true,
                "logon_task": true,
                "config": FILE,
                "config_exists": true,
                "config_valid": true,
                "config_error": null,
                "live_state": "needs_administrator_terminal",
                "message": "fleets, agents and hotkeys need an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status --json",
            })
        );
    }

    #[test]
    fn the_report_is_not_an_error_and_exits_0_whatever_it_finds() {
        let nothing = Fake {
            installed: false,
            version: Some("0.1.2"),
            daemon: false,
            task: false,
            file: None,
            config: Ok(""),
        };
        let broken = Fake {
            config: Ok("{\"cuont\": 3}"),
            ..Fake::all_there()
        };
        for fake in [Fake::all_there(), nothing, broken] {
            let status = gather(&fake, COMMAND);
            for json in [true, false] {
                let (text, code) = report(&status, json);
                assert_eq!(code, 0, "{text}");
                assert!(!text.contains('\n'), "{text}");
            }
            assert_eq!(value(&status).get("error"), None);
            assert_eq!(value(&status)["live_state"], json!(NEEDS_ADMINISTRATOR));
        }
    }

    #[test]
    fn nothing_installed_has_no_version_and_no_config_folder() {
        let status = gather(
            &Fake {
                installed: false,
                // A Settings entry an old uninstall left behind.
                version: Some("0.1.1"),
                daemon: false,
                task: false,
                file: None,
                config: Ok(""),
            },
            "daifuku status",
        );
        assert!(!status.installed);
        assert_eq!(status.installed_version, None);
        assert_eq!(status.config, None);
        assert!(!status.config_valid);
        assert_eq!(
            status.config_error.as_deref(),
            Some("no ProgramData folder")
        );
        assert_eq!(
            status.line(),
            "daifuku not installed, daemon not running, no logon task, no ProgramData folder; fleets, agents and hotkeys need an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status"
        );
    }

    #[test]
    fn the_line_says_what_the_json_says() {
        let status = gather(&Fake::all_there(), "daifuku status");
        assert_eq!(
            status.line(),
            format!(
                "daifuku 0.1.2 installed, daemon running, logon task registered, config valid ({FILE}); fleets, agents and hotkeys need an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status"
            )
        );
        let unknown = gather(
            &Fake {
                version: None,
                ..Fake::all_there()
            },
            "daifuku status",
        );
        assert!(unknown.line().starts_with("daifuku installed, "));
    }

    #[test]
    fn no_config_file_is_the_defaults_and_valid() {
        let status = gather(
            &Fake {
                config: Err(std::io::ErrorKind::NotFound),
                ..Fake::all_there()
            },
            COMMAND,
        );
        assert!(!status.config_exists);
        assert!(status.config_valid);
        assert_eq!(status.config_error, None);
        assert!(
            status
                .line()
                .contains(&format!("no config, the defaults apply ({FILE})")),
            "{}",
            status.line()
        );
    }

    #[test]
    fn a_config_the_daemon_would_not_load_says_why_on_one_line() {
        let status = gather(
            &Fake {
                config: Ok("{\n  \"fleets\": [{ \"name\": \"x\", \"cuont\": 3 }]\n}"),
                ..Fake::all_there()
            },
            COMMAND,
        );
        assert!(status.config_exists);
        assert!(!status.config_valid);
        let error = status.config_error.clone().unwrap();
        assert!(error.contains("cuont"), "{error}");
        assert!(!error.contains('\n'), "{error}");
        assert!(
            status
                .line()
                .contains(&format!("config invalid ({FILE}): {error}")),
            "{}",
            status.line()
        );
    }

    #[test]
    fn a_config_that_cannot_be_read_is_not_valid() {
        let status = gather(
            &Fake {
                config: Err(std::io::ErrorKind::PermissionDenied),
                ..Fake::all_there()
            },
            COMMAND,
        );
        assert!(status.config_exists);
        assert!(!status.config_valid);
        assert!(
            status
                .config_error
                .as_deref()
                .is_some_and(|e| e.starts_with("could not read it: ")),
            "{:?}",
            status.config_error
        );
    }

    #[test]
    fn only_a_daemon_in_this_session_counts() {
        let here = |pid: u32| pid < 100;
        assert!(daemon_in([(4, "System"), (42, "daifukud.exe")], here));
        assert!(daemon_in([(42, "DAIFUKUD.EXE")], here));
        // Another person's daemon, in their own session.
        assert!(!daemon_in([(420, "daifukud.exe")], here));
        // The command line is not the daemon.
        assert!(!daemon_in([(42, "daifuku.exe")], here));
        assert!(!daemon_in([], here));
    }

    #[test]
    fn json_past_ascii_is_escaped_in_the_report() {
        let status = gather(
            &Fake {
                file: Some("C:\\ProgramData\\Daifuku\\Gr\u{fc}ezi.json"),
                ..Fake::all_there()
            },
            COMMAND,
        );
        let (text, _) = report(&status, true);
        assert!(text.is_ascii(), "{text}");
        assert!(text.contains("Gr\\u00fcezi"), "{text}");
    }
}
