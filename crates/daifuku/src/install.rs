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
//! 4. Register this user's logon task that starts the daemon elevated, and
//!    start it. The one task per machine earlier installs registered goes
//!    when it is this user's.
//! 5. Add the hooks to Claude Code's settings and, if Codex is installed,
//!    to Codex's, leaving everything else in each file as it was.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use daifuku_core::agents::{self, Agent, CLAUDE, CODEX};
use daifuku_core::config;
use daifuku_core::config::Config;
use daifuku_core::protocol::{Request, Response, Status, from_line, to_line};
use daifuku_core::task::{LEGACY_TASK_NAME, task_name, task_users, xml};
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
            "removed      {} (it was not locked to administrators, so anyone could have written what was in it)",
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
    let task = task_name(&sid);
    let daemon = to.join("daifukud.exe");
    setup::create_task(&task, &xml(&daemon.to_string_lossy(), &sid), &data)
        .context("could not register the logon task")?;
    println!("logon task   {task}");
    remove_legacy_task(&sid);
    if !options.no_start {
        setup::run_task(&task).context("could not start the daemon")?;
        println!("started      daifukud");
    }

    if !options.no_hooks {
        let exe = to.join("daifuku.exe").to_string_lossy().into_owned();
        for (agent, file) in hook_files() {
            let variable = folder_variable(agent);
            let instead = read_instead(&file, std::env::var_os(variable));
            // Codex only if it is installed: its folder exists, in the
            // profile or where its variable points.
            if agent.name == CODEX.name
                && !file.parent().is_some_and(Path::is_dir)
                && !instead
                    .as_deref()
                    .and_then(Path::parent)
                    .is_some_and(Path::is_dir)
            {
                continue;
            }
            if let Some(other) = &instead {
                println!(
                    "hooks        {} reads {} ({variable}), which install does not write: copy Daifuku's hooks there from {}",
                    agent.name,
                    other.display(),
                    file.display()
                );
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
    // From the config on disk, which an update keeps, so the key named is
    // the one the daemon registers.
    let kept = std::fs::read(&config)
        .and_then(|b| config::decode(&b))
        .map_err(|e| e.to_string())
        .and_then(|t| Config::from_json(&t).map_err(|e| e.to_string()));
    match kept.map(|c| how_to_open(&c)) {
        Ok(Some(how)) if options.no_start => {
            println!("\nAfter your next sign-in, {how} to open your first fleet.");
        }
        Ok(Some(how)) => println!("\nTo open your first fleet, {how}."),
        Ok(None) => {}
        Err(e) => println!("\nThe config does not read, so the daemon runs on its defaults: {e}"),
    }
    Ok(())
}

/// How to open the first fleet in `config`: its key, or the command when it
/// has none. `None` when there is no fleet.
fn how_to_open(config: &Config) -> Option<String> {
    let fleet = config.fleets.first()?;
    Some(match fleet.hotkey.as_ref().and_then(|h| h.parse().ok()) {
        Some(key) => format!("press {key}"),
        None => "run `daifuku open`".to_owned(),
    })
}

pub fn uninstall(purge: bool) -> anyhow::Result<()> {
    if !process::current_is_elevated() {
        bail!("uninstalling needs an administrator terminal");
    }
    stop_daemon();
    let sid = setup::user_sid().context("could not read this user's SID")?;
    setup::delete_task(&task_name(&sid)).context("could not remove the logon task")?;
    println!("removed      logon task");
    remove_legacy_task(&sid);
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
        let (left, removed) = remove_binaries(&dir, &std::env::temp_dir());
        match removed {
            Ok(()) => println!("removed      {}", dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                for file in &left {
                    println!("left         {}", file.display());
                }
                println!("left         {} ({e})", dir.display());
            }
        }
    }
    if purge && let Some(dir) = paths::data_dir() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("could not remove {}", dir.display()))?;
        println!("removed      {}", dir.display());
    } else if let Some(dir) = paths::data_dir() {
        println!(
            "kept         {} (config, logs and saved state; --purge removes them)",
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
    let task = setup::user_sid().map(|sid| task_name(&sid));
    check(
        task.as_deref().is_some_and(setup::task_exists),
        "logon task registered",
        "run `daifuku install`",
    );
    let task = task.unwrap_or_else(|| task_name("<your SID>"));
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
    // In the file the agent reads, which its variable may move out of the
    // one install writes.
    let profile = paths::profile_dir();
    for (agent, file) in hook_files() {
        let variable = folder_variable(agent);
        let instead = read_instead(&file, std::env::var_os(variable));
        let read = instead.as_deref().unwrap_or(&file);
        if agent.name == CODEX.name && !read.parent().is_some_and(Path::is_dir) {
            continue;
        }
        let fix = hooks_fix(agent, read, &exe, profile.as_deref()).map(|fix| match &instead {
            Some(other) => format!(
                "{variable} points {} at {}, which `daifuku install` does not write: copy Daifuku's hooks there from {}",
                agent.name,
                other.display(),
                file.display()
            ),
            None => fix,
        });
        check(
            fix.is_none(),
            &format!("{} hooks present and current", agent.name),
            fix.as_deref().unwrap_or_default(),
        );
    }

    let running = send(Pipe::Hook, "{}\n", Duration::from_millis(200)).is_ok();
    check(
        running,
        "daemon running",
        &format!("sign out and in, or `schtasks /Run /TN {task}`"),
    );

    if !running {
        // Nothing more to ask a daemon that is not there.
    } else if process::current_is_elevated() {
        let mut told = false;
        let reply = patient_status(BUSY_TRIES, || {
            let reply = status_reply(
                to_line(&Request::Status)
                    .map_err(std::io::Error::other)
                    .and_then(|l| send(Pipe::Control, &l, Duration::from_secs(2))),
            );
            if matches!(reply, StatusReply::Busy) && !std::mem::replace(&mut told, true) {
                println!(
                    "--    daemon busy with another command, such as opening a fleet: waiting for it"
                );
            }
            reply
        });
        match reply {
            StatusReply::Status(s) => {
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
            }
            StatusReply::Busy | StatusReply::Stuck => check(
                false,
                "daemon answers",
                &stuck_fix(process::session(), &task),
            ),
            StatusReply::NotElevated => check(
                false,
                "daemon elevated",
                &format!(
                    "the process answering on Daifuku's control pipe is not the elevated daemon: sign out and in, or end it and run `schtasks /Run /TN {task}`"
                ),
            ),
            StatusReply::Silent => check(
                false,
                "daemon answers",
                &format!("restart it: `daifuku stop`, then `schtasks /Run /TN {task}`"),
            ),
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

/// What to do so that `file` holds all of `agent`'s hooks, calling `exe`, or
/// `None` when it does. Running install is the answer, except for a file
/// install leaves alone: one behind a link out of `profile`, or one it cannot
/// read as settings. Install would only refuse it again.
fn hooks_fix(agent: &Agent, file: &Path, exe: &str, profile: Option<&Path>) -> Option<String> {
    let missing = match read_settings(file) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(mut s) => {
                let updated = agents::update_hooks(&mut s, exe);
                agent
                    .add_hooks(&mut s, exe)
                    .map(|added| updated + added)
                    .map_err(|e| {
                        format!(
                            "{}: {e}; repair it, then run `daifuku install`",
                            file.display()
                        )
                    })
            }
            Err(_) => Err(format!(
                "{} is not valid JSON: repair it, then run `daifuku install`",
                file.display()
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(1),
        Err(e) => Err(format!("could not read {}: {e}", file.display())),
    };
    match missing {
        Ok(0) => None,
        _ if profile.is_some_and(|p| !stays_inside(file, p)) => Some(format!(
            "{} leads outside your profile, which `daifuku install` leaves alone: make it a real file in your profile, then run `daifuku install`",
            file.display()
        )),
        Ok(_) => Some("run `daifuku install`".to_owned()),
        Err(fix) => Some(fix),
    }
}

/// What came back when doctor asked the daemon for its status.
#[derive(Debug)]
enum StatusReply {
    /// The status.
    Status(Status),
    /// Nothing yet: another command, such as opening a fleet, holds the one
    /// control pipe.
    Busy,
    /// Busy on every one of [`BUSY_TRIES`] asks: one command has held the
    /// daemon far longer than any should.
    Stuck,
    /// Windows or the pipe client turned the answer away: the process on
    /// the control pipe is not the elevated daemon. Doctor asks only from an
    /// elevated terminal, which the real daemon lets in.
    NotElevated,
    /// No answer, or one that does not read.
    Silent,
}

/// How many times doctor asks a busy daemon before it counts it as stuck.
/// Each ask waits up to two seconds for the pipe, so this is about twenty:
/// opening a fleet takes a few.
const BUSY_TRIES: usize = 10;

/// What to do about a daemon stuck in one command, in Remote Desktop session
/// `session`, whose logon task is `task`.
///
/// It ends only the daemon of this session. The pipes are per session, so
/// each session holds at most one daemon, the signed-in user's, while another
/// person signed in at the same time has their own in their own session. By
/// image name alone it would end theirs too. Not by `%USERNAME%`, which
/// PowerShell does not expand and which would also reach the same user's
/// daemon in another session, nor by process id: the stuck daemon does not
/// answer to say its own.
fn stuck_fix(session: u32, task: &str) -> String {
    format!(
        "it has been busy with one command for over {} seconds, far longer than opening a fleet takes: restart it with `taskkill /F /FI \"SESSION eq {session}\" /IM daifukud.exe`, then `schtasks /Run /TN {task}`",
        BUSY_TRIES * 2
    )
}

/// Asks with `ask` until the answer is not [`StatusReply::Busy`], at most
/// `tries` times; busy every time is [`StatusReply::Stuck`].
fn patient_status(tries: usize, mut ask: impl FnMut() -> StatusReply) -> StatusReply {
    for _ in 0..tries {
        match ask() {
            StatusReply::Busy => {}
            other => return other,
        }
    }
    StatusReply::Stuck
}

/// Reads what [`send`] returned for `status`. A wait that timed out is how
/// it reports a daemon busy with another command.
fn status_reply(reply: std::io::Result<Option<String>>) -> StatusReply {
    match reply {
        Ok(Some(line)) => match from_line::<Response>(&line) {
            Ok(Response::Status(s)) => StatusReply::Status(s),
            _ => StatusReply::Silent,
        },
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => StatusReply::Busy,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => StatusReply::NotElevated,
        _ => StatusReply::Silent,
    }
}

/// The combinations the daemon's status lists as refused. Matched on the
/// whole ending the daemon writes, so a fleet named `refused` is not one.
fn refused_hotkeys(hotkeys: &[String]) -> Vec<&str> {
    hotkeys
        .iter()
        .filter_map(|h| h.strip_suffix(": refused, another program holds it"))
        .collect()
}

/// Removes the one task per machine that earlier installs registered, when
/// it runs for this user, and says so. Another user's stays: it is their
/// autostart until they install again.
fn remove_legacy_task(sid: &str) {
    let Some(definition) = setup::task_xml(LEGACY_TASK_NAME) else {
        return;
    };
    if !runs_for(&task_users(&definition), sid, this_account().as_deref()) {
        return;
    }
    match setup::delete_task(LEGACY_TASK_NAME) {
        Ok(()) => println!("removed      logon task {LEGACY_TASK_NAME} of an earlier install"),
        Err(e) => println!("left         logon task {LEGACY_TASK_NAME}: {e}"),
    }
}

/// This account as `DOMAIN\name`, the other way a task may name its user.
fn this_account() -> Option<String> {
    let domain = std::env::var("USERDOMAIN").ok()?;
    let name = std::env::var("USERNAME").ok()?;
    Some(format!(r"{domain}\{name}"))
}

/// Whether a task whose XML names `users` runs for the user with SID `sid`,
/// whose account is `account`, `DOMAIN\name`. A user named without a domain
/// is this machine's. One that names no user, or anyone else, does not.
fn runs_for(users: &[String], sid: &str, account: Option<&str>) -> bool {
    let is_account = |u: &str| {
        account.is_some_and(|a| {
            u.eq_ignore_ascii_case(a)
                || (!u.contains('\\')
                    && a.rsplit('\\')
                        .next()
                        .is_some_and(|n| u.eq_ignore_ascii_case(n)))
        })
    };
    !users.is_empty()
        && users
            .iter()
            .all(|u| u.eq_ignore_ascii_case(sid) || is_account(u))
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

/// Deletes the installed binaries and then their folder, and returns the
/// files that stayed and what became of the folder.
///
/// A program that runs cannot be deleted, only renamed: this one, run from
/// there, or a daemon that did not stop. Such a file is moved into `aside`,
/// the temp folder, to be cleaned up with it, so the folder can still go.
/// When `aside` is on another drive, the file is renamed next to the folder
/// instead, under a name of its own, and Windows deletes that name at the
/// next restart. Never the install path itself: an install before the
/// restart would lose its new files. Tried on every file that will not go,
/// not just on this program's own path, which may be spelled in another
/// case or as a short name.
fn remove_binaries(dir: &Path, aside: &Path) -> (Vec<PathBuf>, std::io::Result<()>) {
    let mut left = Vec::new();
    for name in BINARIES {
        let file = dir.join(name);
        if let Err(e) = std::fs::remove_file(&file)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            let old = format!("daifuku-old-{}-{name}", std::process::id());
            if std::fs::rename(&file, aside.join(&old)).is_ok() {
                continue;
            }
            let beside = dir.parent().map(|p| p.join(&old));
            match beside {
                Some(b) if std::fs::rename(&file, &b).is_ok() => {
                    let _ = setup::delete_at_restart(&b);
                }
                _ => left.push(file),
            }
        }
    }
    (left, std::fs::remove_dir(dir))
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

/// The variable that points `agent` at another folder than the one in the
/// profile, which install writes: Claude Code's `CLAUDE_CONFIG_DIR`, Codex's
/// `CODEX_HOME`.
fn folder_variable(agent: &Agent) -> &'static str {
    if agent.name == CODEX.name {
        "CODEX_HOME"
    } else {
        "CLAUDE_CONFIG_DIR"
    }
}

/// The file an agent reads in place of `file` when `folder`, the value of
/// its [`folder_variable`], names another folder than the one `file` is in;
/// `None` when it reads `file`. Install writes only the file in the profile:
/// the variable is the user's to set, and install runs as administrator.
fn read_instead(file: &Path, folder: Option<OsString>) -> Option<PathBuf> {
    let folder = PathBuf::from(folder.filter(|f| !f.is_empty())?);
    let default = file.parent()?;
    let same = match (
        std::fs::canonicalize(&folder),
        std::fs::canonicalize(default),
    ) {
        (Ok(a), Ok(b)) => a == b,
        // Not there to ask the file system: compared as written, in any
        // case and with or without a separator at the end.
        _ => {
            let key = |p: &Path| {
                p.components()
                    .map(|c| c.as_os_str().to_ascii_lowercase())
                    .collect::<Vec<_>>()
            };
            key(&folder) == key(default)
        }
    };
    if same {
        None
    } else {
        Some(folder.join(file.file_name()?))
    }
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
/// in the user's profile, and only with the user's own rights.
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
    // a desktop shell to borrow them from, as on a build server, the file is
    // left alone: the checks above alone do not close that gap.
    process::as_shell_user(run).unwrap_or_else(|e| Err(shell_refused(e, path)))
}

/// Why an agent's settings file was not edited when the user's own rights
/// could not be borrowed for it.
fn shell_refused(e: std::io::Error, path: &Path) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow!(
            "{} left untouched: no desktop shell is running to change it with your own rights; run this from an administrator terminal on your signed-in desktop, or change Daifuku's hooks in it by hand",
            path.display()
        )
    } else {
        anyhow::Error::new(e).context(format!("could not edit {}", path.display()))
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
    fn the_installer_names_the_key_the_config_binds() {
        let mut c = Config::default();
        assert_eq!(
            how_to_open(&c).as_deref(),
            Some("press ctrl + alt + return")
        );
        c.fleets[0].hotkey = Some(config::HotkeyText::new("Alt+CTRL+A"));
        assert_eq!(how_to_open(&c).as_deref(), Some("press ctrl + alt + a"));
        c.fleets[0].hotkey = None;
        assert_eq!(how_to_open(&c).as_deref(), Some("run `daifuku open`"));
        c.fleets.clear();
        assert_eq!(how_to_open(&c), None);
    }

    #[test]
    fn without_a_desktop_shell_a_settings_file_is_left_alone() {
        let file = Path::new(r"C:\Users\ada\.claude\settings.json");
        let none = shell_refused(
            std::io::Error::new(std::io::ErrorKind::NotFound, "no desktop shell"),
            file,
        );
        let text = format!("{none:#}");
        assert!(text.contains("left untouched"), "{text}");
        assert!(text.contains("signed-in desktop"), "{text}");
        assert!(
            text.contains(r"C:\Users\ada\.claude\settings.json"),
            "{text}"
        );
        let other = shell_refused(std::io::Error::other("refused"), file);
        assert_eq!(
            format!("{other:#}"),
            r"could not edit C:\Users\ada\.claude\settings.json: refused"
        );
    }

    #[test]
    fn only_this_users_earlier_task_is_theirs_to_remove() {
        fn users(u: &[&str]) -> Vec<String> {
            u.iter().map(|&s| s.to_owned()).collect()
        }
        let sid = "S-1-5-21-1-2-3-1001";
        assert!(runs_for(&users(&[sid, sid]), sid, None));
        assert!(runs_for(&users(&["s-1-5-21-1-2-3-1001"]), sid, None));
        assert!(runs_for(&users(&[r"PC\ada"]), sid, Some(r"pc\Ada")));
        assert!(runs_for(&users(&["ada", sid]), sid, Some(r"PC\Ada")));
        assert!(!runs_for(&users(&[r"WORK\ada"]), sid, Some(r"PC\Ada")));
        assert!(!runs_for(
            &users(&["S-1-5-21-1-2-3-1002"]),
            sid,
            Some(r"PC\x")
        ));
        assert!(!runs_for(&users(&[sid, "S-1-5-21-1-2-3-1002"]), sid, None));
        assert!(!runs_for(&users(&[r"PC\other"]), sid, None));
        assert!(
            !runs_for(&[], sid, Some(r"PC\x")),
            "a task that names no one"
        );
    }

    /// The users of a real `\Daifuku\Daemon` task as Task Scheduler gives
    /// them back (daifuku-win's fixture): the principal as a SID, the logon
    /// trigger as `PC\ada`. Install removes it for that user only.
    #[test]
    fn an_earlier_installs_real_task_is_removed_for_its_user_only() {
        let legacy = include_str!("../../daifuku-win/tests/fixtures/legacy-task.xml");
        let users = task_users(legacy);
        let sid = "S-1-5-21-1-2-3-1001";
        assert!(runs_for(&users, sid, Some(r"PC\ada")));
        assert!(runs_for(&users, sid, Some(r"pc\ADA")), "names ignore case");
        assert!(
            !runs_for(&users, sid, None),
            "without the account the trigger's user is unknown, so it stays"
        );
        assert!(!runs_for(&users, "S-1-5-21-1-2-3-1002", Some(r"PC\bob")));
        assert!(!runs_for(&users, sid, Some(r"WORK\ada")));
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

    /// What `send` returns while another command holds the control pipe.
    #[test]
    fn a_daemon_busy_with_another_command_is_busy_not_silent() {
        let busy = std::io::Error::new(std::io::ErrorKind::TimedOut, "busy");
        assert!(matches!(status_reply(Err(busy)), StatusReply::Busy));
        let gone = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        assert!(matches!(status_reply(Err(gone)), StatusReply::Silent));
        // What the pipe client says of a server that is not elevated, and
        // what Windows says when it refuses the open.
        let impostor = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the process answering on Daifuku's control pipe is not the elevated daemon",
        );
        assert!(matches!(
            status_reply(Err(impostor)),
            StatusReply::NotElevated
        ));
        let refused = std::io::Error::from_raw_os_error(5);
        assert!(matches!(
            status_reply(Err(refused)),
            StatusReply::NotElevated
        ));
        let garbled = Ok(Some("not json\n".to_owned()));
        assert!(matches!(status_reply(garbled), StatusReply::Silent));
        let line = to_line(&Response::Status(Status::default())).unwrap();
        assert!(matches!(
            status_reply(Ok(Some(line))),
            StatusReply::Status(_)
        ));
    }

    #[test]
    fn a_stuck_daemon_is_ended_only_in_this_session() {
        let fix = stuck_fix(2, r"\Daifuku\Daemon-S-1-5-21-1-2-3-1001");
        assert!(
            fix.contains(r#"`taskkill /F /FI "SESSION eq 2" /IM daifukud.exe`"#),
            "{fix}"
        );
        assert!(!fix.contains('%'), "nothing a shell must expand: {fix}");
        assert!(
            fix.ends_with(r"`schtasks /Run /TN \Daifuku\Daemon-S-1-5-21-1-2-3-1001`"),
            "{fix}"
        );
    }

    #[test]
    fn a_daemon_busy_on_every_ask_is_stuck() {
        let mut asks = 0;
        let stuck = patient_status(3, || {
            asks += 1;
            StatusReply::Busy
        });
        assert!(matches!(stuck, StatusReply::Stuck), "{stuck:?}");
        assert_eq!(asks, 3);

        let mut asks = 0;
        let answered = patient_status(3, || {
            asks += 1;
            if asks < 3 {
                StatusReply::Busy
            } else {
                StatusReply::Status(Status::default())
            }
        });
        assert!(matches!(answered, StatusReply::Status(_)), "{answered:?}");

        let mut asks = 0;
        let silent = patient_status(3, || {
            asks += 1;
            StatusReply::Silent
        });
        assert!(matches!(silent, StatusReply::Silent));
        assert_eq!(asks, 1, "only busy is asked again");
    }

    /// A file held open without leave to delete it can be neither deleted
    /// nor renamed, so it and the folder stay, and say so.
    #[test]
    fn a_binary_that_cannot_go_is_named_and_keeps_its_folder() {
        use std::os::windows::fs::OpenOptionsExt;
        /// `FILE_SHARE_READ`.
        const FILE_SHARE_READ: u32 = 1;
        let base = std::env::temp_dir().join(format!("daifuku-remove-{}", std::process::id()));
        let dir = base.join("Daifuku");
        let aside = base.join("aside");
        std::fs::create_dir_all(&aside).unwrap();
        let fill = || {
            std::fs::create_dir_all(&dir).unwrap();
            for name in BINARIES {
                std::fs::write(dir.join(name), "").unwrap();
            }
        };

        fill();
        let (left, removed) = remove_binaries(&dir, &aside);
        let gone = !dir.exists();

        fill();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(dir.join("daifukud.exe"))
            .unwrap();
        let (held_left, held_removed) = remove_binaries(&dir, &aside);
        let stayed = dir.join("daifukud.exe").is_file() && !dir.join("daifuku.exe").exists();
        drop(held);

        let (_, again) = remove_binaries(&dir, &aside);
        let (_, nothing) = remove_binaries(&dir, &aside);
        std::fs::remove_dir_all(&base).unwrap();

        assert!(left.is_empty() && removed.is_ok() && gone);
        assert_eq!(held_left, [dir.join("daifukud.exe")]);
        assert!(held_removed.is_err() && stayed);
        assert!(again.is_ok(), "{again:?}");
        assert_eq!(
            nothing.map_err(|e| e.kind()),
            Err(std::io::ErrorKind::NotFound)
        );
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
    fn a_variable_that_moves_an_agent_elsewhere_is_noticed() {
        let base = std::env::temp_dir().join(format!("daifuku-moved-{}", std::process::id()));
        let own = base.join(".claude");
        let other = base.join("work");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let file = own.join("settings.json");
        let shouted = OsString::from(own.to_string_lossy().to_uppercase());
        let gone = base.join("gone");
        let mut gone_slash = gone.clone().into_os_string();
        gone_slash.push(r"\");

        let unset = read_instead(&file, None);
        let empty = read_instead(&file, Some(OsString::new()));
        let same = read_instead(&file, Some(shouted));
        let same_unmade = read_instead(&gone.join("settings.json"), Some(gone_slash));
        let moved = read_instead(&file, Some(other.clone().into_os_string()));
        std::fs::remove_dir_all(&base).unwrap();

        assert_eq!((unset, empty), (None, None));
        assert_eq!(same, None, "the same folder in other letters");
        assert_eq!(same_unmade, None, "a folder not made yet, as written");
        assert_eq!(moved, Some(other.join("settings.json")));
    }

    /// Doctor sends to install only what install would change.
    #[test]
    fn doctor_names_a_settings_file_install_would_refuse() {
        let base = std::env::temp_dir().join(format!("daifuku-fix-{}", std::process::id()));
        let profile = base.join("profile");
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(profile.join("own")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let made = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(profile.join(".claude"))
            .arg(&outside)
            .output()
            .unwrap();
        let exe = r"C:\Program Files\Daifuku\daifuku.exe";
        let mut hooked = serde_json::json!({});
        CLAUDE.add_hooks(&mut hooked, exe).unwrap();
        let own = profile.join("own").join("settings.json");
        let linked = profile.join(".claude").join("settings.json");
        let fix = |file: &Path| hooks_fix(&CLAUDE, file, exe, Some(&profile));

        let missing = fix(&own);
        std::fs::write(&own, "{ \"theme\": ").unwrap();
        let broken = fix(&own);
        std::fs::write(&own, hooked.to_string()).unwrap();
        let current = fix(&own);
        let linked_missing = fix(&linked);
        std::fs::write(outside.join("settings.json"), hooked.to_string()).unwrap();
        let linked_current = fix(&linked);
        let _ = std::fs::remove_dir(profile.join(".claude"));
        std::fs::remove_dir_all(&base).unwrap();

        assert!(made.status.success(), "{made:?}");
        assert_eq!(missing.as_deref(), Some("run `daifuku install`"));
        assert!(broken.is_some_and(|f| f.contains("is not valid JSON: repair it")));
        assert_eq!(current, None);
        assert!(linked_missing.is_some_and(|f| f.contains("leads outside your profile")));
        assert_eq!(linked_current, None, "hooks added by hand count");
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
