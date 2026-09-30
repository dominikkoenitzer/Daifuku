//! `daifuku install`, `daifuku uninstall` and `daifuku doctor`.
//!
//! Install, in order, each step safe to repeat:
//!
//! 1. Stop a running daemon, so its files can be replaced.
//! 2. Copy `daifuku.exe` and `daifukud.exe` next to this program into
//!    `%ProgramFiles%\Daifuku`, where only administrators can change them.
//!    The daemon runs elevated, so a binary an ordinary process could swap
//!    would hand that process administrator rights at the next logon.
//! 3. Lock `%ProgramData%\Daifuku` to administrators, for the same reason,
//!    and write a starter config there if there is none. A folder there that
//!    an earlier install did not lock is removed first, config and all.
//! 4. Register the logon task that starts the daemon elevated, and start it.
//! 5. Add the hooks to Claude Code's settings and, if Codex is installed,
//!    to Codex's, leaving everything else in each file as it was.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use daifuku_core::agents::{self, Agent, CLAUDE, CODEX};
use daifuku_core::config;
use daifuku_core::config::Config;
use daifuku_core::protocol::{Request, to_line};
use daifuku_core::task::{TASK_NAME, xml};
use daifuku_win::pipe::{Pipe, send};
use daifuku_win::{paths, process, setup, terminal};

const BINARIES: [&str; 2] = ["daifuku.exe", "daifukud.exe"];

/// What `install` should skip.
pub struct Options {
    /// Leave Claude Code's and Codex's settings alone.
    pub no_hooks: bool,
    /// Register the task but do not start the daemon now.
    pub no_start: bool,
}

pub fn install(options: &Options) -> anyhow::Result<()> {
    if !process::current_is_elevated() {
        bail!("installing needs an administrator terminal");
    }
    let from = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .context("no folder for this program")?;
    let to = paths::install_dir().context("no Program Files folder")?;
    let data = paths::data_dir().context("no ProgramData folder")?;

    stop_daemon();

    if from != to {
        std::fs::create_dir_all(&to)?;
        for name in BINARIES {
            let src = from.join(name);
            if !src.is_file() {
                bail!("{} is missing next to this program", name);
            }
            copy_retrying(&src, &to.join(name))?;
        }
        println!("installed    {}", to.display());
    }

    let removed =
        setup::harden_dir(&data).with_context(|| format!("could not lock {}", data.display()))?;
    if removed {
        println!(
            "removed      {} (not locked by an earlier install, so nothing in it was trusted)",
            data.display()
        );
    }
    let config = data.join("daifuku.json");
    if config.is_file() {
        println!("kept config  {}", config.display());
    } else {
        std::fs::write(&config, starter_config())?;
        println!("wrote config {}", config.display());
    }

    let sid = setup::user_sid().context("could not read this user's SID")?;
    let daemon = to.join("daifukud.exe");
    setup::create_task(TASK_NAME, &xml(&daemon.to_string_lossy(), &sid), &data)
        .context("could not register the logon task")?;
    println!("logon task   {TASK_NAME}");
    if !options.no_start {
        setup::run_task(TASK_NAME).context("could not start the daemon")?;
        println!("started      daifukud");
    }

    if !options.no_hooks {
        let exe = to.join("daifuku.exe").to_string_lossy().into_owned();
        for (agent, file) in hook_files() {
            // Codex only if it is installed: its folder exists.
            if agent.name == CODEX.name && !file.parent().is_some_and(Path::is_dir) {
                continue;
            }
            let mut updated = 0;
            let edited = edit_json(&file, |s| {
                updated = agents::update_hooks(s, &exe);
                Ok(updated + agent.add_hooks(s, &exe).map_err(|e| anyhow!(e))?)
            });
            match edited {
                Ok(0) => println!("hooks        {} already has them", agent.name),
                Ok(n) => {
                    if updated > 0 {
                        println!("hooks        updated {updated} in {}", file.display());
                    }
                    if n > updated {
                        println!("hooks        added {} to {}", n - updated, file.display());
                    }
                }
                Err(e) => println!("hooks        {} not changed: {e:#}", agent.name),
            }
        }
    }

    if terminal::find().is_none() {
        println!(
            "\nWindows Terminal is not installed; fleets need it (winget install Microsoft.WindowsTerminal)."
        );
    }
    println!("\nPress Ctrl+Alt+Enter to open your first fleet.");
    Ok(())
}

pub fn uninstall(purge: bool) -> anyhow::Result<()> {
    if !process::current_is_elevated() {
        bail!("uninstalling needs an administrator terminal");
    }
    stop_daemon();
    setup::delete_task(TASK_NAME).context("could not remove the logon task")?;
    println!("removed      logon task");
    for (agent, file) in hook_files() {
        if !file.is_file() {
            continue;
        }
        match edit_json(&file, |s| Ok(agents::remove_hooks(s))) {
            Ok(0) => {}
            Ok(1) => println!("removed      1 hook from {}", file.display()),
            Ok(n) => println!("removed      {n} hooks from {}", file.display()),
            Err(e) => println!("hooks        {} not changed: {e:#}", agent.name),
        }
    }
    if let Some(dir) = paths::install_dir() {
        let me = std::env::current_exe().ok();
        for name in BINARIES {
            let file = dir.join(name);
            if Some(&file) == me.as_ref() {
                // A running program cannot delete itself: rename it out of
                // the way so the folder can go, and let the rename's target
                // be cleaned up with the temp folder.
                let _ = std::fs::rename(
                    &file,
                    std::env::temp_dir().join(format!("daifuku-old-{}.exe", std::process::id())),
                );
            } else {
                let _ = std::fs::remove_file(&file);
            }
        }
        let _ = std::fs::remove_dir(&dir);
        println!("removed      {}", dir.display());
    }
    if purge && let Some(dir) = paths::data_dir() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("could not remove {}", dir.display()))?;
        println!("removed      {}", dir.display());
    } else if let Some(dir) = paths::data_dir() {
        println!(
            "kept         {} (config and logs; --purge removes them)",
            dir.display()
        );
    }
    Ok(())
}

/// Checks everything that can stop Daifuku from working, and says what to do
/// about each problem.
pub fn doctor() -> bool {
    let mut ok = true;
    let mut check = |good: bool, label: &str, fix: &str| {
        if good {
            println!("ok    {label}");
        } else {
            ok = false;
            println!("FIX   {label}: {fix}");
        }
    };

    let installed =
        paths::install_dir().is_some_and(|d| BINARIES.iter().all(|b| d.join(b).is_file()));
    check(
        installed,
        "installed in Program Files",
        "run `daifuku install` from an administrator terminal",
    );
    check(
        setup::task_exists(TASK_NAME),
        "logon task registered",
        "run `daifuku install`",
    );
    check(
        terminal::find().is_some(),
        "Windows Terminal found",
        "winget install Microsoft.WindowsTerminal",
    );

    let config_path = paths::config_file();
    let config = config_path
        .as_ref()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default());
    match config.as_deref().map(Config::from_json) {
        Some(Ok(_)) => check(true, "config valid", ""),
        Some(Err(e)) => check(false, "config valid", &e.to_string()),
        None => check(false, "config valid", "no ProgramData folder"),
    }

    for (agent, file) in hook_files() {
        if agent.name == CODEX.name && !file.parent().is_some_and(Path::is_dir) {
            continue;
        }
        let present = read_settings(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|mut s| agent.add_hooks(&mut s, "daifuku.exe") == Ok(0));
        check(
            present,
            &format!("{} hooks present", agent.name),
            "run `daifuku install`, or add them by hand (see the README)",
        );
    }

    let running = send(Pipe::Hook, "{}\n", Duration::from_millis(200)).is_ok();
    check(
        running,
        "daemon running",
        "sign out and in, or `schtasks /Run /TN \\Daifuku\\Daemon`",
    );

    if !running {
        // Nothing more to ask a daemon that is not there.
    } else if process::current_is_elevated() {
        let status = to_line(&Request::Status)
            .ok()
            .and_then(|l| {
                send(Pipe::Control, &l, Duration::from_secs(2))
                    .ok()
                    .flatten()
            })
            .and_then(|r| {
                daifuku_core::protocol::from_line::<daifuku_core::protocol::Response>(&r).ok()
            });
        if let Some(daifuku_core::protocol::Response::Status(s)) = status {
            check(
                s.elevated,
                "daemon elevated",
                "start it through the logon task, not by hand",
            );
            let refused = refused_hotkeys(&s.hotkeys);
            check(
                refused.is_empty(),
                "hotkeys registered",
                &format!(
                    "another program holds {}; pick others in the config",
                    refused.join(", ")
                ),
            );
        } else {
            check(
                false,
                "daemon answers",
                r"restart it: `daifuku stop`, then `schtasks /Run /TN \Daifuku\Daemon`",
            );
        }
    } else {
        println!(
            "--    elevation and hotkeys: run doctor from an administrator terminal to check those too"
        );
    }
    ok
}

/// The combinations the daemon's status lists as refused. Matched on the
/// whole ending the daemon writes, so a fleet named `refused` is not one.
fn refused_hotkeys(hotkeys: &[String]) -> Vec<&str> {
    hotkeys
        .iter()
        .filter_map(|h| h.strip_suffix(": refused, another program holds it"))
        .collect()
}

/// Asks a running daemon to stop and waits a moment for it to exit.
fn stop_daemon() {
    let Ok(line) = to_line(&Request::Stop) else {
        return;
    };
    if send(Pipe::Control, &line, Duration::from_millis(500)).is_err() {
        return;
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if send(Pipe::Hook, "{}\n", Duration::from_millis(50)).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Copies, retrying for a few seconds while a stopping daemon still holds the
/// file open.
fn copy_retrying(from: &Path, to: &Path) -> anyhow::Result<()> {
    let start = Instant::now();
    loop {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) if start.elapsed() < Duration::from_secs(5) => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(e).with_context(|| format!("could not copy {}", to.display())),
        }
    }
}

/// Every agent and the file its hooks live in.
fn hook_files() -> Vec<(&'static Agent, PathBuf)> {
    let mut out = Vec::new();
    if let Some(p) = paths::claude_settings() {
        out.push((&CLAUDE, p));
    }
    if let Some(p) = paths::codex_hooks() {
        out.push((&CODEX, p));
    }
    out
}

/// Reads a settings file as text, without the byte order mark many Windows
/// editors put first, which JSON parsers refuse.
fn read_settings(path: &Path) -> std::io::Result<String> {
    let text = std::fs::read_to_string(path)?;
    Ok(match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_owned(),
        None => text,
    })
}

/// Reads a JSON settings file, applies `edit`, and writes the file back only
/// if something changed. A missing file counts as `{}`. Returns what `edit`
/// returned.
fn edit_json(
    path: &Path,
    edit: impl FnOnce(&mut serde_json::Value) -> anyhow::Result<usize>,
) -> anyhow::Result<usize> {
    let text = match read_settings(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".to_owned(),
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
    };
    let mut settings: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON, left untouched", path.display()))?;
    let changed = edit(&mut settings)?;
    if changed > 0 {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = serde_json::to_string_pretty(&settings)?;
        out.push('\n');
        std::fs::write(path, out)?;
    }
    Ok(changed)
}

/// The config a fresh install starts with: the defaults, spelled out, with
/// the schema linked so an editor completes and checks every key.
fn starter_config() -> String {
    let mut c = Config {
        schema: Some(
            "https://raw.githubusercontent.com/dominikkoenitzer/Daifuku/main/schema.json"
                .to_owned(),
        ),
        ..Config::default()
    };
    c.fleets = vec![config::Fleet::default()];
    let mut s = serde_json::to_string_pretty(&c).unwrap_or_else(|_| "{}".to_owned());
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_config_is_valid_and_links_the_schema() {
        let text = starter_config();
        let c = Config::from_json(&text).unwrap();
        assert!(c.schema.is_some());
        assert_eq!(c.fleets.len(), 1);
    }

    #[test]
    fn only_refused_hotkeys_count_as_refused() {
        let hotkeys = [
            "Ctrl+Alt+1: open refused".to_owned(),
            "Ctrl+Alt+N: next waiting agent".to_owned(),
            "Ctrl+Alt+S: refused, another program holds it".to_owned(),
        ];
        assert_eq!(refused_hotkeys(&hotkeys), ["Ctrl+Alt+S"]);
    }

    #[test]
    fn a_settings_file_with_a_byte_order_mark_is_still_json() {
        let dir = std::env::temp_dir().join(format!("daifuku-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, "\u{feff}{\"theme\": \"dark\"}\n").unwrap();
        let edited = edit_json(&file, |s| {
            s["hooks"] = serde_json::json!({});
            Ok(1)
        });
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(edited.unwrap(), 1);
        let settings: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(settings["theme"], "dark");
    }
}
