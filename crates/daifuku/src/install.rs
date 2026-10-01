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
    // A standard user's prompt answered with an administrator's password
    // runs this as the administrator: the task, the hooks and the settings
    // would all be theirs, and the user at the desktop would get nothing.
    // Windows 11's Administrator protection does the same to every
    // administrator terminal: it runs as a hidden account of its own.
    if let Err(e) = process::as_shell_user(|| ())
        && e.kind() == std::io::ErrorKind::PermissionDenied
    {
        bail!(
            "this terminal runs as another account than the one signed in, as it does when a standard user's prompt takes an administrator's password, or under Windows Administrator protection, which Daifuku does not support: install from an administrator terminal of the account that runs the agents"
        );
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
            let edited = edit_agent_file(&file, |s| {
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
        match edit_agent_file(&file, |s| Ok(agents::remove_hooks(s))) {
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

    // No file is the default config; a file that cannot be read is not.
    let config =
        paths::config_file().map(
            |p| match std::fs::read(&p).and_then(|b| config::decode(&b)) {
                Ok(text) => Config::from_json(&text).map_err(|e| e.to_string()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
                Err(e) => Err(format!("{}: {e}", p.display())),
            },
        );
    match config {
        Some(Ok(_)) => check(true, "config valid", ""),
        Some(Err(e)) => check(false, "config valid", &e),
        None => check(false, "config valid", "no ProgramData folder"),
    }

    // The hooks must all be there and call the installed copy: a hook left
    // calling an older or deleted one reports nothing.
    let exe = paths::install_dir()
        .map(|d| d.join("daifuku.exe").to_string_lossy().into_owned())
        .unwrap_or_default();
    for (agent, file) in hook_files() {
        if agent.name == CODEX.name && !file.parent().is_some_and(Path::is_dir) {
            continue;
        }
        let present = read_settings(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|mut s| {
                agents::update_hooks(&mut s, &exe) == 0 && agent.add_hooks(&mut s, &exe) == Ok(0)
            });
        check(
            present,
            &format!("{} hooks present and current", agent.name),
            "run `daifuku install`",
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
            check(
                s.version == env!("CARGO_PKG_VERSION"),
                "daemon up to date",
                &format!(
                    "the daemon runs {} and this daifuku is {}: run `daifuku install` from the newer one, which also restarts the daemon",
                    s.version,
                    env!("CARGO_PKG_VERSION")
                ),
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
    if let Some(logs) = paths::log_dir(true) {
        println!("logs  {}", logs.display());
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

/// [`edit_json`] for an agent's settings file, only while the file really is
/// in the user's profile.
///
/// The installer runs as administrator and the profile is the user's to
/// change: a link at `.claude` or at the file itself could otherwise point
/// the edit at a file only administrators may write.
fn edit_agent_file(
    path: &Path,
    edit: impl FnOnce(&mut serde_json::Value) -> anyhow::Result<usize>,
) -> anyhow::Result<usize> {
    let profile = paths::profile_dir().context("no profile folder")?;
    let run = || {
        if !stays_inside(path, &profile) {
            bail!(
                "{} leads outside your profile, left untouched",
                path.display()
            );
        }
        edit_json(path, edit)
    };
    // With the user's own rights, so a link swapped in while the file is
    // edited cannot lead the write anywhere the user may not write. Without
    // a desktop shell to borrow them from, as on a build server, the checks
    // above are all there is.
    let mut run = Some(run);
    match process::as_shell_user(|| run.take().map(|r| r())) {
        Ok(Some(result)) => result,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match run.take() {
            Some(r) => r(),
            None => bail!("the edit of {} did not run", path.display()),
        },
        Ok(None) => bail!("the edit of {} did not run", path.display()),
        Err(e) => Err(e).with_context(|| format!("could not edit {}", path.display())),
    }
}

/// Whether `path`, with every link on the way followed, is inside `root`.
/// A path that does not exist yet is judged by the nearest folder above it
/// that does.
fn stays_inside(path: &Path, root: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    path.ancestors()
        .find_map(|p| std::fs::canonicalize(p).ok())
        .is_some_and(|real| real.starts_with(&root))
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
        write_whole(path, &out).with_context(|| format!("could not write {}", path.display()))?;
    }
    Ok(changed)
}

/// Writes `text` into a new file next to `path` and renames it over `path`,
/// so a crash half way leaves the old file, never one cut short. A link at
/// `path` is followed, so the file it points to is the one replaced.
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    /// `FILE_FLAG_OPEN_REPARSE_POINT`.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".daifuku-new");
    let new = path.with_file_name(name);
    // A new file, never one already there: the profile is the user's, and a
    // link planted at this name would otherwise take the write elsewhere.
    // A leftover from a crash is removed first; removing a link removes the
    // link, not what it points to.
    let _ = std::fs::remove_file(&new);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        // A name that is already a link then fails instead of being
        // followed.
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&new)
        .and_then(|mut f| {
            f.write_all(text.as_bytes())?;
            f.sync_all()
        });
    let result = written.and_then(|()| std::fs::rename(&new, &path));
    if result.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    result
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

    #[test]
    fn a_junction_out_of_the_root_is_not_inside_it() {
        let base = std::env::temp_dir().join(format!("daifuku-inside-{}", std::process::id()));
        let root = base.join("profile");
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let made = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(root.join(".claude"))
            .arg(&outside)
            .output()
            .unwrap();
        let inside = stays_inside(&root.join("x").join("settings.json"), &root);
        let through = stays_inside(&root.join(".claude").join("settings.json"), &root);
        let _ = std::fs::remove_dir(root.join(".claude"));
        std::fs::remove_dir_all(&base).unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(inside, "a file yet to be made under the root");
        assert!(!through, "a junction that leads out of the root");
    }

    #[test]
    fn a_folder_that_only_starts_like_the_root_is_not_inside_it() {
        let base = std::env::temp_dir().join(format!("daifuku-sibling-{}", std::process::id()));
        let root = base.join("profile");
        let sibling = base.join("profile2");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let beside = stays_inside(&sibling.join("s.json"), &root);
        let rootless = stays_inside(&root.join("s.json"), &base.join("no-such-root"));
        std::fs::remove_dir_all(&base).unwrap();
        assert!(!beside, "a sibling whose name begins with the root's");
        assert!(!rootless, "a root that does not exist holds nothing");
    }

    /// A second link to the file shows whether it was rewritten in place,
    /// which a crash half way through would leave cut short, or replaced
    /// whole.
    #[test]
    fn a_settings_file_is_replaced_whole_and_only_when_changed() {
        let dir = std::env::temp_dir().join(format!("daifuku-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        let link = dir.join("link.json");
        std::fs::write(&file, "{ \"theme\": \"dark\" }").unwrap();
        std::fs::hard_link(&file, &link).unwrap();
        let unchanged = edit_json(&file, |_| Ok(0));
        let text_unchanged = std::fs::read_to_string(&file).unwrap();
        let changed = edit_json(&file, |s| {
            s["hooks"] = serde_json::json!({});
            Ok(1)
        });
        let text = std::fs::read_to_string(&file).unwrap();
        let linked = std::fs::read_to_string(&link).unwrap();
        let created = edit_json(&dir.join("new.json"), |_| Ok(1));
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(unchanged.unwrap(), 0);
        assert_eq!(text_unchanged, "{ \"theme\": \"dark\" }", "not written");
        assert_eq!(changed.unwrap(), 1);
        assert!(text.contains("\"hooks\""));
        assert_eq!(linked, "{ \"theme\": \"dark\" }", "replaced, not rewritten");
        created.unwrap();
        assert_eq!(names.len(), 3, "no file left behind: {names:?}");
    }
}
