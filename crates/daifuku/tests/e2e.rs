//! End to end, on a real desktop: the real daemon, a real console window, the
//! real hook reporting from inside it, and the border that results.
//!
//! Opt-in, because it starts a daemon and opens windows. One test at a time,
//! as CI runs them: each starts a daemon of its own, a session has room for
//! one, and all of them write the same config file. In PowerShell:
//!
//! ```text
//! cargo build --workspace
//! $env:DAIFUKU_E2E = '1'
//! cargo test -p daifuku --test e2e -- --nocapture --test-threads 1
//! ```
//!
//! A daemon that already runs in this session, an installed one included,
//! has to be stopped first with `daifuku stop`; the tests refuse to start
//! beside it. While they run, the hooks of every agent in this session report
//! to the test's daemon, so run them where no agent of your own is at work.
//!
//! CI runs it on a hosted Windows runner and fails the job unless the final
//! tally line is printed, because a test that returns early still passes.

#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use daifuku_core::protocol::{Response, Status, from_line};
use daifuku_win::window;

const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

fn enabled() -> bool {
    std::env::var("DAIFUKU_E2E").is_ok_and(|v| v == "1")
}

fn cli() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_daifuku"))
}

fn daemon_exe() -> PathBuf {
    cli().with_file_name("daifukud.exe")
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daifuku-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `daifuku status --json`, if the daemon answers.
fn status() -> Option<Status> {
    let out = Command::new(cli())
        .args(["status", "--json"])
        .output()
        .ok()?;
    match from_line::<Response>(&String::from_utf8_lossy(&out.stdout)).ok()? {
        Response::Status(s) => Some(s),
        _ => None,
    }
}

fn wait_for<T>(what: &str, limit: Duration, mut probe: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = probe() {
            return v;
        }
        assert!(start.elapsed() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A hook event file the console feeds to `daifuku hook`.
fn event(dir: &Path, name: &str) -> PathBuf {
    let file = dir.join(format!("{name}.json"));
    std::fs::write(
        &file,
        format!(r#"{{"session_id":"e2e","hook_event_name":"{name}"}}"#),
    )
    .unwrap();
    file
}

/// The status of the daemon that answers, if it is the one started on
/// `config`.
fn ours(config: &str) -> Option<Status> {
    status().filter(|s| s.config.starts_with(config))
}

/// Starts a daemon on `config` and waits until it is the one that answers.
///
/// Another daemon in this session, such as an installed one, holds the
/// pipes: the new one exits at once, and every command and key of the test
/// would go to the other one and the real agents it watches. So the test
/// refuses to start beside one, before there is anything to clean up.
fn start(config: &Path) -> (Cleanup, Status) {
    assert!(
        status().is_none(),
        "a daifukud already runs in this session; stop it first with `daifuku stop`"
    );
    let daemon = Command::new(daemon_exe())
        .arg("--config")
        .arg(config)
        .spawn()
        .unwrap();
    let mut cleanup = Cleanup(vec![daemon], config.display().to_string());
    let first = wait_for(
        "this test's daemon to answer",
        Duration::from_secs(15),
        || {
            assert!(
                cleanup.0[0].try_wait().unwrap().is_none(),
                "this test's daifukud exited; another one holds the pipes"
            );
            ours(&cleanup.1)
        },
    );
    (cleanup, first)
}

/// What a test started, undone even when it fails half way. The second
/// field is the test's config file, as its daemon names it in `status`.
struct Cleanup(Vec<Child>, String);

impl Drop for Cleanup {
    fn drop(&mut self) {
        // A test that failed half way leaves its terminals open, and the
        // demo's agents in them run until they are closed. Only the test's
        // own daemon gets these commands: any other keeps its fleets and
        // runs on.
        if ours(&self.1).is_some() {
            for fleet in ["e2e", "demo", "lead", "crew"] {
                let _ = Command::new(cli()).args(["close", fleet]).output();
            }
            let _ = Command::new(cli()).arg("stop").output();
        }
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[test]
fn a_hook_in_a_real_console_colours_its_window_and_the_border_follows_it_away() {
    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    assert!(
        daemon_exe().is_file(),
        "build the workspace first: {} is missing",
        daemon_exe().display()
    );
    let dir = scratch();
    let config = dir.join("daifuku.json");
    // No hotkeys: a runner's desktop is shared with nothing, but a developer's
    // machine running this by hand is, and must not lose its keys.
    std::fs::write(
        &config,
        r#"{"fleets":[{"name":"e2e","hotkey":null}],"hotkeys":{"next_waiting":null,"snap":null,"close":null}}"#,
    )
    .unwrap();
    let mut steps = 0;

    let (mut cleanup, first) = start(&config);
    assert!(first.fleets.is_empty());
    steps += 1;

    // A console of its own, like a terminal an agent runs in: the hook
    // reports waiting, working and done, a second apart, and the console then
    // stays open until the test closes it. Its title is the test's own, so
    // only its window counts: every agent in this session reports to the
    // test's daemon as well, and one of them may be waiting too.
    let tag = format!("daifuku-e2e-{}", std::process::id());
    let hook = cli();
    let script = format!(
        "title {tag} & \"{h}\" hook < \"{w}\" & ping -n 3 127.0.0.1 >nul & \"{h}\" hook < \"{k}\" & ping -n 3 127.0.0.1 >nul & \"{h}\" hook < \"{d}\" & ping -n 60 127.0.0.1 >nul",
        h = hook.display(),
        w = event(&dir, "PermissionRequest").display(),
        k = event(&dir, "PreToolUse").display(),
        d = event(&dir, "Stop").display(),
    );
    // `/s` and one pair of outer quotes: without them cmd strips the first
    // and the last quote of a line that starts with one, which breaks every
    // quoted path in it. Measured on the first CI run.
    let console = Command::new("cmd.exe")
        .args(["/d", "/s", "/c"])
        .raw_arg(format!("\"{script}\""))
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .unwrap();
    cleanup.0.push(console);

    let waiting = wait_for("the waiting state", Duration::from_secs(15), || {
        status()?
            .agents
            .into_iter()
            .find(|a| a.state.name() == "waiting" && a.title.contains(&tag))
    });
    let w = waiting.window;
    assert!(
        window::exists(w) && window::is_shown(w),
        "the agent's window is a real, visible one"
    );
    steps += 1;

    // The border: one frame, around exactly that window.
    let frame = window::frame(w).unwrap();
    let border = wait_for("a border around the window", Duration::from_secs(5), || {
        window::top_level().into_iter().find(|&b| {
            window::class(b) == "DaifukuBorder"
                && window::is_shown(b)
                && window::outer(b).is_some_and(|r| {
                    (r.left - frame.left).abs() <= 12
                        && (r.top - frame.top).abs() <= 12
                        && (r.right - frame.right).abs() <= 12
                        && (r.bottom - frame.bottom).abs() <= 12
                })
        })
    });
    steps += 1;

    for next in ["working", "done"] {
        wait_for(next, Duration::from_secs(15), || {
            status()?
                .agents
                .into_iter()
                .find(|a| a.window == w && a.state.name() == next)
        });
        steps += 1;
    }

    // Closing the console takes the agent and its border with it.
    let mut console = cleanup.0.pop().unwrap();
    let _ = console.kill();
    let _ = console.wait();
    let _ = window::close(w);
    wait_for("the agent to be forgotten", Duration::from_secs(10), || {
        status().filter(|s| s.agents.iter().all(|a| a.window != w))
    });
    wait_for("the border to go", Duration::from_secs(5), || {
        (!window::is_shown(border)).then_some(())
    });
    steps += 2;

    let reply = Command::new(cli()).arg("stop").output().unwrap();
    assert!(String::from_utf8_lossy(&reply.stdout).contains("stopping"));
    let mut daemon = cleanup.0.pop().unwrap();
    wait_for("the daemon to exit", Duration::from_secs(10), || {
        daemon.try_wait().ok().flatten()
    });
    steps += 1;

    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

/// Runs `daifuku` with arguments and returns what it printed.
fn run(args: &[&str]) -> String {
    let out = Command::new(cli()).args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

#[test]
fn a_fleet_opens_in_its_grid_snaps_back_and_closes() {
    use daifuku_core::grid::{Gaps, Grid};
    use daifuku_core::monitor::MonitorInfo;

    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    daifuku_win::dpi::per_monitor_v2();
    let dir = scratch();
    let config = dir.join("daifuku.json");
    // Each terminal writes a file through a command with the characters that
    // have to survive two command lines, Windows Terminal's and PowerShell's:
    // double quotes, a semicolon and `{n}`.
    let said = dir.join("said");
    std::fs::create_dir_all(&said).unwrap();
    let command = format!(
        r#"Set-Content -LiteralPath '{}\said-{{n}}.txt' -Value "terminal {{n}}; ""quoted""""#,
        said.display()
    );
    let fleets = serde_json::json!({
        "fleets": [{
            "name": "e2e", "count": 4, "monitor": "primary", "command": command,
            "admin": true, "no_profile": true, "hotkey": null
        }],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, fleets.to_string()).unwrap();
    let mut steps = 0;
    let (mut cleanup, _) = start(&config);

    let opened = run(&["open", "e2e"]);
    assert!(opened.contains("opened e2e (4 terminals"), "{opened}");
    let fleet = wait_for("the fleet in the status", Duration::from_secs(10), || {
        status()?
            .fleets
            .into_iter()
            .find(|f| f.name == "e2e" && f.windows.len() == 4)
    });
    steps += 1;

    // Every terminal ran its command as written, with its own number.
    for n in 1..=4 {
        let file = said.join(format!("said-{n}.txt"));
        // Only a whole file: Set-Content ends it with a line break.
        let read = || {
            std::fs::read_to_string(&file)
                .ok()
                .filter(|t| t.ends_with('\n'))
        };
        let start = Instant::now();
        while read().is_none() && start.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let Some(text) = read() else {
            // What PowerShell was given says which of the two command lines
            // lost what.
            let shells = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    "Get-CimInstance Win32_Process | Where-Object Name -in 'pwsh.exe','powershell.exe' | ForEach-Object CommandLine",
                ])
                .output()
                .unwrap();
            panic!(
                "terminal {n} did not run its command; the shells' command lines:\n{}",
                String::from_utf8_lossy(&shells.stdout)
            );
        };
        assert_eq!(text.trim_end(), format!(r#"terminal {n}; "quoted""#));
    }
    steps += 1;

    // Every terminal exactly in its cell, by its visible frame.
    let monitors: Vec<MonitorInfo> = daifuku_win::monitor::monitors();
    let primary = monitors.iter().find(|m| m.primary).unwrap();
    let cells = Grid::plan(4, primary.work, Gaps::default(), None);
    for (i, &w) in fleet.windows.iter().enumerate() {
        assert_eq!(
            window::frame(w),
            Some(cells[i]),
            "terminal {} is not in its cell",
            i + 1
        );
    }
    steps += 1;

    // Pushed out of its cell, a snap puts it back.
    let first = fleet.windows[0];
    let moved = cells[0];
    window::place(
        first,
        daifuku_core::Rect::new(
            moved.left + 40,
            moved.top + 30,
            moved.right - 40,
            moved.bottom - 30,
        ),
    );
    assert_ne!(
        window::frame(first),
        Some(cells[0]),
        "the move did not happen"
    );
    let snapped = run(&["snap"]);
    assert!(snapped.contains("snapped 4 terminals"), "{snapped}");
    assert_eq!(
        window::frame(first),
        Some(cells[0]),
        "snap did not put it back"
    );
    steps += 1;

    // A terminal closed by hand comes back in its own cell, and the others
    // stay where they are.
    let before = fleet.windows.clone();
    let second = before[1];
    assert!(window::close(second));
    wait_for(
        "the second terminal to close",
        Duration::from_secs(15),
        || (!window::exists(second)).then_some(()),
    );
    let reopened = run(&["open", "e2e"]);
    assert!(reopened.contains("opened e2e (4 terminals"), "{reopened}");
    let fleet = wait_for("the reopened fleet", Duration::from_secs(10), || {
        status()?
            .fleets
            .into_iter()
            .find(|f| f.name == "e2e" && f.windows.len() == 4)
    });
    assert_ne!(fleet.windows[1], second, "no new terminal took the slot");
    for i in [0, 2, 3] {
        assert_eq!(fleet.windows[i], before[i], "terminal {} moved", i + 1);
    }
    for (i, &w) in fleet.windows.iter().enumerate() {
        assert_eq!(
            window::frame(w),
            Some(cells[i]),
            "terminal {} is not in its cell after the reopen",
            i + 1
        );
    }
    steps += 1;

    // Closing the fleet takes every terminal down.
    let closed = run(&["close", "e2e"]);
    assert!(closed.contains("closed 4 terminals"), "{closed}");
    wait_for("the terminals to close", Duration::from_secs(15), || {
        fleet
            .windows
            .iter()
            .all(|&w| !window::exists(w))
            .then_some(())
    });
    steps += 1;

    // Opened again, `close --all` takes it down as well, and then has
    // nothing left to close.
    let opened = run(&["open", "e2e"]);
    assert!(opened.contains("opened e2e (4 terminals"), "{opened}");
    let fleet = wait_for("the fleet open again", Duration::from_secs(10), || {
        status()?
            .fleets
            .into_iter()
            .find(|f| f.name == "e2e" && f.windows.len() == 4)
    });
    let closed = run(&["close", "--all"]);
    assert!(closed.contains("closed 4 terminals"), "{closed}");
    wait_for("every terminal to close", Duration::from_secs(15), || {
        fleet
            .windows
            .iter()
            .all(|&w| !window::exists(w))
            .then_some(())
    });
    let again = run(&["close", "--all"]);
    assert!(again.contains("no fleet is open"), "{again}");
    steps += 1;

    let _ = run(&["stop"]);
    let mut daemon = cleanup.0.pop().unwrap();
    wait_for("the daemon to exit", Duration::from_secs(10), || {
        daemon.try_wait().ok().flatten()
    });
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

#[test]
fn a_fleet_on_an_account_signs_every_terminal_in_to_it_and_no_other_fleet() {
    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    let dir = scratch();
    let config = dir.join("daifuku.json");
    // A quote in the folder, which has to reach the shell as a quote and not
    // end the string it is set in.
    let account = dir.join("it's work");
    let (work, home) = (dir.join("work"), dir.join("home"));
    for d in [&work, &home] {
        std::fs::create_dir_all(d).unwrap();
    }
    // Each terminal writes what it was given into a file of its own, named
    // by its shell's process.
    let command = |into: &Path| {
        format!(
            r#"Set-Content -LiteralPath ('{}\' + $PID + '.txt') -Value "[$env:CLAUDE_CONFIG_DIR][$env:CODEX_HOME]""#,
            into.display()
        )
    };
    // Crew's first terminal also opens a peer as an agent would, naming no
    // fleet: it has to take crew's, not e2e's, the first in the config.
    let inner = dir.join("inner");
    let crew = format!(
        "{}; if (-not (Test-Path -LiteralPath '{}')) {{ New-Item -ItemType File -Path '{}' | Out-Null; & '{}' spawn --dir '{}' --label 'e2e inner' }}",
        command(&home),
        inner.display(),
        inner.display(),
        cli().display(),
        home.display()
    );
    let json = serde_json::json!({
        "accounts": { "Work": { "claude": account } },
        "fleets": [
            {
                "name": "e2e", "count": 2, "monitor": "primary", "area": "left",
                "command": command(&work), "account": "work",
                "admin": true, "no_profile": true, "hotkey": null
            },
            {
                "name": "crew", "count": 1, "monitor": "primary", "area": "right",
                "command": crew,
                "admin": true, "no_profile": true, "hotkey": null
            }
        ],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, json.to_string()).unwrap();
    let mut steps = 0;
    // Peers of its own, so a failed run leaves the real ones as they were.
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let (mut cleanup, _) = start_with(&config, Some(&state), &[]);

    // Whole files only: Set-Content ends each with a line break.
    let said = |folder: &Path| -> Vec<String> {
        std::fs::read_dir(folder)
            .map(|entries| {
                entries
                    .filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
                    .filter(|t| t.ends_with('\n'))
                    .map(|t| t.trim_end().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let expected = format!("[{}][]", account.display());

    for fleet in ["e2e", "crew"] {
        let opened = run(&["open", fleet]);
        assert!(opened.contains(&format!("opened {fleet}")), "{opened}");
    }
    let lines = wait_for(
        "both terminals of e2e to write",
        Duration::from_secs(30),
        || {
            let lines = said(&work);
            (lines.len() == 2).then_some(lines)
        },
    );
    assert!(
        lines.iter().all(|l| *l == expected),
        "{lines:?}, not {expected}"
    );
    steps += 1;

    // The fleet with no account leaves the variables as they were: unset on
    // a runner, so the agent reads the folder in the profile. So does the
    // peer its terminal opened without naming a fleet.
    let lines = wait_for(
        "crew's terminal and its peer to write",
        Duration::from_secs(45),
        || {
            let lines = said(&home);
            (lines.len() == 2).then_some(lines)
        },
    );
    assert!(
        lines.iter().all(|l| !l.contains("it's work")),
        "crew got e2e's account: {lines:?}"
    );
    assert_eq!(said(&work).len(), 2, "crew's peer opened in e2e");
    steps += 1;

    // A peer the fleet's agent opens signs in to the same account.
    let peer = spawn(&[
        "--fleet",
        "e2e",
        "--dir",
        &work.display().to_string(),
        "--label",
        "e2e account",
    ]);
    let lines = wait_for("the peer to write", Duration::from_secs(30), || {
        let lines = said(&work);
        (lines.len() == 3).then_some(lines)
    });
    assert!(
        lines.iter().all(|l| *l == expected),
        "{lines:?}, not {expected}"
    );
    steps += 1;

    let _ = run(&["close", "--terminal", "e2e account"]);
    let _ = run(&["close", "--terminal", "e2e inner"]);
    let _ = run(&["close", "--all"]);
    wait_for("the peer to close", Duration::from_secs(15), || {
        (!window::exists(peer)).then_some(())
    });
    let _ = run(&["stop"]);
    let mut daemon = cleanup.0.pop().unwrap();
    wait_for("the daemon to exit", Duration::from_secs(10), || {
        daemon.try_wait().ok().flatten()
    });
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

/// What Claude Code hands its status line: the account has used `five_hour`
/// percent of a 5-hour window an hour from its reset, and 35 of its week.
fn status_line_input(dir: &Path, name: &str, five_hour: f64) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let file = dir.join(format!("{name}.json"));
    let json = serde_json::json!({
        "session_id": "e2e",
        "model": { "id": "e2e" },
        "rate_limits": {
            "five_hour": { "used_percentage": five_hour, "resets_at": now + 3600 },
            "seven_day": { "used_percentage": 35, "resets_at": now + 3 * 86_400 }
        }
    });
    std::fs::write(&file, json.to_string()).unwrap();
    file
}

#[test]
fn an_account_s_status_line_shows_its_use_and_warns_on_its_borders() {
    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    let dir = scratch();
    let config = dir.join("daifuku.json");
    let account = dir.join("work account");
    std::fs::create_dir_all(&account).unwrap();
    let json = serde_json::json!({
        "accounts": { "work": { "claude": account, "warn": 77 } },
        "fleets": [{ "name": "e2e", "hotkey": null }],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, json.to_string()).unwrap();
    let mut steps = 0;
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let (mut cleanup, first) = start_with(&config, Some(&state), &[]);
    assert!(first.usage.is_empty());
    steps += 1;

    // An agent's console on the account: its hook says it works, then its
    // status line reports 80 % of the 5-hour window, past the warn at 77.
    let tag = format!("daifuku-e2e-usage-{}", std::process::id());
    let printed = dir.join("line.txt");
    let script = format!(
        "title {tag} & \"{h}\" hook < \"{k}\" & \"{h}\" status-line < \"{s}\" > \"{p}\" & ping -n 60 127.0.0.1 >nul",
        h = cli().display(),
        k = event(&dir, "PreToolUse").display(),
        s = status_line_input(&dir, "high", 80.4).display(),
        p = printed.display(),
    );
    let console = Command::new("cmd.exe")
        .args(["/d", "/s", "/c"])
        .raw_arg(format!("\"{script}\""))
        .env("CLAUDE_CONFIG_DIR", &account)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .unwrap();
    cleanup.0.push(console);

    let agent = wait_for(
        "the agent to show its account's warning",
        Duration::from_secs(20),
        || {
            status()?
                .agents
                .into_iter()
                .find(|a| a.title.contains(&tag) && a.warning)
        },
    );
    assert_eq!(agent.account.as_deref(), Some("work"));
    assert_eq!(agent.state.name(), "working");
    steps += 1;

    let s = status().unwrap();
    let [usage] = s.usage.as_slice() else {
        panic!("one account, not {:?}", s.usage)
    };
    assert_eq!(usage.account.as_deref(), Some("work"));
    assert!(same_dir(&usage.folder, &account), "{}", usage.folder);
    assert_eq!(usage.warn, Some(77));
    assert!(usage.warning);
    let five = usage.five_hour.unwrap();
    assert_eq!(five.used, 80);
    assert!(
        five.resets_in_seconds
            .is_some_and(|s| s > 3000 && s <= 3600),
        "{five:?}"
    );
    // An hour from the end of five: 80 % gone by, so on pace give or take.
    assert!(five.pace.is_some_and(|p| p.abs() <= 2), "{five:?}");
    assert_eq!(usage.seven_day.unwrap().used, 35);
    steps += 1;

    // The status line itself printed the same numbers, as one line.
    let line = wait_for("the status line to print", Duration::from_secs(5), || {
        std::fs::read_to_string(&printed)
            .ok()
            .filter(|t| t.ends_with('\n'))
    });
    assert!(
        line.starts_with("5h 80% (") && line.contains(", week 35% ("),
        "{line}"
    );
    let shown = run(&["status"]);
    assert!(
        shown.contains("\nusage\n  work  5h 80%,") && shown.contains("past its warn at 77%"),
        "{shown}"
    );
    steps += 1;

    // A later report under the warn, from anywhere on the account, takes the
    // warning off its agents; quiet, it prints nothing.
    let quiet = Command::new(cli())
        .args(["status-line", "--quiet"])
        .env("CLAUDE_CONFIG_DIR", &account)
        .stdin(std::fs::File::open(status_line_input(&dir, "low", 10.0)).unwrap())
        .output()
        .unwrap();
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty(), "{quiet:?}");
    wait_for("the warning to go", Duration::from_secs(10), || {
        let s = status()?;
        let agent = s.agents.iter().find(|a| a.title.contains(&tag))?;
        (!agent.warning && s.usage.iter().all(|u| !u.warning)).then_some(())
    });
    steps += 1;

    // The numbers outlive the daemon.
    let mut console = cleanup.0.pop().unwrap();
    let _ = console.kill();
    let _ = console.wait();
    stop(&mut cleanup);
    let (mut cleanup, again) = start_with(&config, Some(&state), &[]);
    let usage = again.usage.first().expect("the account's use was kept");
    assert_eq!(usage.five_hour.unwrap().used, 10);
    assert!(!usage.warning);
    steps += 1;

    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

/// A window as a line: handle, class, title and, for the foreground, which
/// of its children has the keyboard and whether it is in a menu.
fn describe(w: u64) -> String {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetGUIThreadInfo, GetWindowThreadProcessId,
    };

    let h = HWND(w as usize as *mut core::ffi::c_void);
    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap(),
        ..Default::default()
    };
    // SAFETY: plain calls on a handle and a sized struct.
    let known = unsafe {
        let thread = GetWindowThreadProcessId(h, None);
        thread != 0 && GetGUIThreadInfo(thread, &raw mut info).is_ok()
    };
    let raw = |h: HWND| h.0 as usize as u64;
    let focus = if known {
        let f = raw(info.hwndFocus);
        format!(
            ", keyboard {f:#x} ({}), active {:#x}, flags {:#x}",
            window::class(f),
            raw(info.hwndActive),
            info.flags.0
        )
    } else {
        String::new()
    };
    format!("{w:#x} {} {:?}{focus}", window::class(w), window::title(w))
}

/// Presses (`true`) or releases keys, each with its scan code as a keyboard
/// sends it: Windows Terminal reads the scan code and drops a key without
/// one. Returns how many of the events Windows took.
fn keys(strokes: &[(u16, u16, bool)]) -> u32 {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VIRTUAL_KEY,
    };

    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|&(vk, scan, down)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: scan,
                    dwFlags: if down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    },
                    ..Default::default()
                },
            },
        })
        .collect();
    // SAFETY: inputs is a valid array of INPUT with the right size.
    unsafe { SendInput(&inputs, i32::try_from(size_of::<INPUT>()).unwrap()) }
}

/// Virtual key and scan code of the keys the tests press.
const ENTER: (u16, u16) = (0x0D, 0x1C);
const CTRL: (u16, u16) = (0x11, 0x1D);
const ALT: (u16, u16) = (0x12, 0x38);
const F11: (u16, u16) = (0x7A, 0x57);

fn press_enter() -> u32 {
    keys(&[(ENTER.0, ENTER.1, true), (ENTER.0, ENTER.1, false)])
}

/// Ctrl+Alt+F11 the way a hand plays it: the chord goes down, F11 comes up,
/// and the modifiers only a moment later, after the hotkey has fired.
fn press_hotkey() -> u32 {
    let mut sent = keys(&[
        (CTRL.0, CTRL.1, true),
        (ALT.0, ALT.1, true),
        (F11.0, F11.1, true),
    ]);
    std::thread::sleep(Duration::from_millis(60));
    sent += keys(&[(F11.0, F11.1, false)]);
    std::thread::sleep(Duration::from_millis(120));
    sent + keys(&[(ALT.0, ALT.1, false), (CTRL.0, CTRL.1, false)])
}

/// The demo's windows whose agents wait. A daemon that stopped answering
/// fails the test: an empty answer would read as "every agent was approved".
fn waiting() -> Vec<u64> {
    demo_waiting(wait_for(
        "the daemon to answer",
        Duration::from_secs(5),
        status,
    ))
}

/// The windows in `s` whose agents wait and that belong to the demo. Only
/// the demo's: an agent of the developer's own in this session reports to
/// the test's daemon as well, and Enter must never reach it.
fn demo_waiting(s: Status) -> Vec<u64> {
    let demo: Vec<u64> = s
        .fleets
        .into_iter()
        .filter(|f| f.name == "demo")
        .flat_map(|f| f.windows)
        .collect();
    s.agents
        .into_iter()
        .filter(|a| a.state.name() == "waiting" && demo.contains(&a.window))
        .map(|a| a.window)
        .collect()
}

fn print_agents() {
    for a in status().map(|s| s.agents).unwrap_or_default() {
        println!(
            "  agent {:#x} {:?}: {} for {} s",
            a.window,
            a.title,
            a.state.name(),
            a.for_seconds
        );
    }
}

#[test]
fn next_brings_each_waiting_agent_up_and_enter_approves_it() {
    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    let dir = scratch();
    let config = dir.join("daifuku.json");
    // A next key no other program is likely to hold, so registering it
    // cannot fail on a runner.
    std::fs::write(
        &config,
        r#"{"fleets":[],"hotkeys":{"next_waiting":"ctrl + alt + f11","snap":null,"close":null}}"#,
    )
    .unwrap();
    let mut steps = 0;
    let (mut cleanup, first) = start(&config);
    println!("hotkeys: {:?}", first.hotkeys);

    // The demo: six scripted agents, two of which ask for approval in their
    // first task and then wait for Enter, and two more in their second.
    let opened = run(&["demo"]);
    assert!(opened.contains("opened demo"), "{opened}");
    wait_for("two waiting agents", Duration::from_secs(60), || {
        (waiting().len() >= 2).then_some(())
    });
    steps += 1;

    // Four times: `next` brings a waiting agent's terminal to the front, and
    // Enter typed there approves it. Twice from the command line, then twice
    // from the key, which fires while its modifiers are still held down.
    let mut previous = None;
    for round in 1..=4 {
        let by_key = round > 2;
        if round == 3 {
            wait_for("two more waiting agents", Duration::from_secs(60), || {
                (waiting().len() >= 2).then_some(())
            });
        }
        let queue = waiting();
        println!("round {round}: waiting {queue:#x?}");
        print_agents();
        let before = window::foreground();
        println!("  before next: {}", describe(before));
        let target = if by_key {
            let sent = press_hotkey();
            println!("  key: sent {sent} of 6 key events");
            let start = Instant::now();
            while window::foreground() == before && start.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(20));
            }
            // The daemon checks that the switch settled for 80 ms.
            std::thread::sleep(Duration::from_millis(200));
            window::foreground()
        } else {
            let reply = run(&["next"]);
            println!("  next: {}", reply.trim());
            window::foreground()
        };
        println!("  after next: {}", describe(target));
        assert!(
            queue.contains(&target) && previous != Some(target),
            "round {round}: next left {} in front, not a waiting agent's terminal",
            describe(target)
        );
        previous = Some(target);
        steps += 1;

        let sent = press_enter();
        println!("  sent {sent} of 2 key events");
        let start = Instant::now();
        let mut left = false;
        while start.elapsed() < Duration::from_secs(10) {
            if !waiting().contains(&target) {
                left = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("  after enter: {}", describe(window::foreground()));
        print_agents();
        assert!(
            left,
            "round {round}: Enter did not reach the agent in {target:#x}"
        );
        steps += 1;
    }

    let _ = run(&["close", "demo"]);
    let _ = run(&["stop"]);
    let mut daemon = cleanup.0.pop().unwrap();
    wait_for("the daemon to exit", Duration::from_secs(10), || {
        daemon.try_wait().ok().flatten()
    });
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

/// Without a desktop, so it runs in every `cargo test`: the rounds above only
/// ever count, bring up and press Enter in the demo's own terminals.
#[test]
fn only_the_demos_waiting_agents_count() {
    use daifuku_core::protocol::{AgentWindow, FleetStatus};
    use daifuku_core::state::AgentState;

    let agent = |window, state| AgentWindow {
        window,
        title: String::new(),
        state,
        for_seconds: 0,
        label: None,
        fleet: None,
        session_id: None,
        cwd: None,
        last_tool: None,
        last_message: None,
        transcript_path: None,
        account: None,
        warning: false,
    };
    let s = Status {
        fleets: vec![FleetStatus {
            name: "demo".into(),
            monitor: String::new(),
            windows: vec![1, 2, 3],
        }],
        agents: vec![
            agent(1, AgentState::Waiting),
            agent(2, AgentState::Working),
            // A real agent beside the test, waiting longest.
            agent(9, AgentState::Waiting),
            agent(3, AgentState::Waiting),
        ],
        ..Status::default()
    };
    assert_eq!(demo_waiting(s), vec![1, 3]);
}

/// Starts a daemon on `config` with `vars` in its environment, keeping its
/// state in `state` when given, and waits until it is the one that answers.
fn start_with(config: &Path, state: Option<&Path>, vars: &[(&str, &str)]) -> (Cleanup, Status) {
    assert!(
        status().is_none(),
        "a daifukud already runs in this session; stop it first with `daifuku stop`"
    );
    let mut command = Command::new(daemon_exe());
    command.arg("--config").arg(config);
    if let Some(dir) = state {
        command.arg("--state").arg(dir);
    }
    command.envs(vars.iter().copied());
    let mut cleanup = Cleanup(vec![command.spawn().unwrap()], config.display().to_string());
    let first = wait_for(
        "this test's daemon to answer",
        Duration::from_secs(15),
        || {
            assert!(
                cleanup.0.last_mut().unwrap().try_wait().unwrap().is_none(),
                "this test's daifukud exited; another one holds the pipes"
            );
            ours(&cleanup.1)
        },
    );
    (cleanup, first)
}

/// Stops the test's daemon and waits for it to exit.
fn stop(cleanup: &mut Cleanup) {
    let _ = run(&["stop"]);
    let mut daemon = cleanup.0.pop().unwrap();
    wait_for("the daemon to exit", Duration::from_secs(10), || {
        daemon.try_wait().ok().flatten()
    });
}

/// A check known to fail, run and reported without failing the test, so
/// the suite shows the day it starts to pass.
fn expect_failure(what: &str, passed: bool) {
    if passed {
        println!("e2e: expected failure now passes: {what}");
    } else {
        println!("e2e: expected failure: {what}");
    }
}

/// The script a peer's terminal runs. It writes what it was started with to
/// `ran-<its arguments>.txt` beside it, as [`Ran`] reads it, and reports the
/// hook event in `peer-event.json` from inside its terminal, as an agent
/// would, or `peer-event-<its arguments>.json` when there is one. With a
/// file `ping-me` beside it, it first starts a hidden `ping`
/// that would outlive it; with `agent-me`, a second PowerShell on its
/// console plays the agent, reports the event two seconds later instead and
/// stays, its process id in `agent-pid.txt`. With `leftover-me` as well, that
/// agent first starts a hidden `ping` of its own, its id in
/// `leftover-pid.txt`.
fn peer_script(dir: &Path) -> PathBuf {
    let script = dir.join("peer.ps1");
    let text = format!(
        r#"$here = '{dir}'
$out = Join-Path $here ('ran-' + ($args -join '-') + '.txt')
$claude = @(Get-ChildItem env: | Where-Object Name -like 'CLAUDE*').Count
$class = (Get-Process -Id $PID).PriorityClass
$stale = [int](Test-Path env:DAIFUKU_E2E_STALE)
$fresh = [int](($env:PATH -like '*\System32*') -and -not ($env:PATH -like '*daifuku-e2e-stale*'))
$ping = 0
if (Test-Path (Join-Path $here 'ping-me')) {{
    $ping = (Start-Process ping.exe -ArgumentList '-n','120','127.0.0.1' -PassThru -WindowStyle Hidden).Id
}}
$event = Join-Path $here 'peer-event.json'
$own = Join-Path $here ('peer-event-' + ($args -join '-') + '.json')
if ($args.Count -gt 0 -and (Test-Path $own)) {{ $event = $own }}
if (Test-Path (Join-Path $here 'agent-me')) {{
    $leftover = ''
    if (Test-Path (Join-Path $here 'leftover-me')) {{
        $leftover = "Set-Content -LiteralPath '$here\leftover-pid.txt' -Value (Start-Process ping.exe -ArgumentList '-n','120','127.0.0.1' -PassThru -WindowStyle Hidden).Id;"
    }}
    $agent = Start-Process powershell.exe -NoNewWindow -PassThru -ArgumentList '-NoProfile','-Command',"Start-Sleep 2; $leftover Get-Content -Raw '$event' | & '{cli}' hook; Start-Sleep 120"
    Set-Content -LiteralPath (Join-Path $here 'agent-pid.txt') -Value $agent.Id
}} elseif (Test-Path $event) {{
    Get-Content -Raw $event | & '{cli}' hook
}}
Set-Content -LiteralPath $out -Value ("$PWD|" + ($args -join ' ') + "|$claude|$PID|$class|$stale|$fresh|$ping")
"#,
        dir = dir.display(),
        cli = cli().display()
    );
    std::fs::write(&script, text).unwrap();
    script
}

/// What a peer's script wrote.
struct Ran {
    /// The folder it started in.
    folder: String,
    /// Its arguments, joined by spaces.
    args: String,
    /// How many `CLAUDE` variables it saw.
    claude: String,
    /// Its process id.
    pid: u32,
    /// Its priority class, `Normal`, `BelowNormal` and so on.
    class: String,
    /// `1` when it saw the daemon's own `DAIFUKU_E2E_STALE`.
    stale: String,
    /// `1` when its `PATH` is the one saved in the registry, not the
    /// daemon's.
    fresh_path: String,
    /// The hidden `ping` it started, 0 for none.
    ping: u32,
}

/// What a peer's script wrote: folder, arguments, `CLAUDE` variables seen
/// and process id.
fn ran(dir: &Path, args: &str) -> (String, String, String, u32) {
    let r = ran_full(dir, args);
    (r.folder, r.args, r.claude, r.pid)
}

/// Everything a peer's script wrote, once it wrote it.
fn ran_full(dir: &Path, args: &str) -> Ran {
    let file = dir.join(format!("ran-{args}.txt"));
    let text = wait_for(
        &format!("{} to be written", file.display()),
        Duration::from_secs(30),
        || {
            std::fs::read_to_string(&file)
                .ok()
                .filter(|t| t.ends_with('\n'))
        },
    );
    let parts: Vec<&str> = text.trim_end().split('|').collect();
    assert_eq!(parts.len(), 8, "{text}");
    Ran {
        folder: parts[0].to_owned(),
        args: parts[1].to_owned(),
        claude: parts[2].to_owned(),
        pid: parts[3].parse().unwrap(),
        class: parts[4].to_owned(),
        stale: parts[5].to_owned(),
        fresh_path: parts[6].to_owned(),
        ping: parts[7].parse().unwrap(),
    }
}

/// Whether process `pid` still runs.
fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!(" {pid} "))
}

/// The peers file a daemon keeps in `state`, one per user.
fn peers_file(state: &Path) -> PathBuf {
    kept_file(state, "peers")
}

/// The file of fleets and agents a daemon saves in `state`, one per user.
fn state_file(state: &Path) -> PathBuf {
    kept_file(state, "state")
}

/// The file `what` a daemon keeps in `state`.
fn kept_file(state: &Path, what: &str) -> PathBuf {
    std::fs::read_dir(state)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(what) && n.ends_with(".json"))
        })
        .unwrap_or_else(|| panic!("a {what} file"))
}

/// A border shown around `w`'s frame, if there is one.
fn border_around(w: u64) -> Option<u64> {
    let frame = window::frame(w)?;
    window::top_level().into_iter().find(|&b| {
        window::class(b) == "DaifukuBorder"
            && window::is_shown(b)
            && window::outer(b).is_some_and(|r| {
                (r.left - frame.left).abs() <= 12
                    && (r.top - frame.top).abs() <= 12
                    && (r.right - frame.right).abs() <= 12
                    && (r.bottom - frame.bottom).abs() <= 12
            })
    })
}

/// A config with one fleet whose terminals run the peer script.
fn peer_config(dir: &Path) -> PathBuf {
    let config = dir.join("daifuku.json");
    let command = format!("& '{}'", peer_script(dir).display());
    let json = serde_json::json!({
        "fleets": [{
            "name": "e2e", "count": 1, "command": command,
            "admin": true, "no_profile": true, "hotkey": null
        }],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, json.to_string()).unwrap();
    config
}

/// `daifuku spawn` with `args`, and the window it opened.
fn spawn(args: &[&str]) -> u64 {
    let mut all = vec!["spawn"];
    all.extend_from_slice(args);
    all.push("--json");
    let reply = run(&all);
    match from_line::<Response>(&reply) {
        Ok(Response::Spawned { window, .. }) => window,
        other => panic!("spawn answered {other:?}: {reply}"),
    }
}

/// Whether two folders are the same, whichever form of their path each one
/// is written in: a runner's temp folder has a short 8.3 name.
fn same_dir(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Narrower than this, Windows Terminal widens a window it is given. A
/// runner's screen is 1024 pixels wide, so half of it is narrower.
const TERMINAL_MIN_WIDTH: i32 = 480;

/// Asserts that terminal `w` is in `cell`: exactly, or for a cell narrower
/// than Windows Terminal allows, exactly at its left, top and bottom edges
/// and at most a few pixels wider, which is then listed as known.
fn assert_in_cell(w: u64, cell: daifuku_core::Rect, what: &str) {
    let frame = window::frame(w).unwrap_or_else(|| panic!("{what} has no frame"));
    if frame == cell {
        return;
    }
    let widened = cell.width() < TERMINAL_MIN_WIDTH
        && (frame.left, frame.top, frame.bottom) == (cell.left, cell.top, cell.bottom)
        && (0..=12).contains(&(frame.width() - cell.width()));
    assert!(widened, "{what} is at {frame:?}, not in its cell {cell:?}");
    expect_failure(
        &format!(
            "{what} fills its cell exactly; Windows Terminal widened the {} px cell to {} px",
            cell.width(),
            frame.width()
        ),
        false,
    );
}

/// The JSON `daifuku` printed.
fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn spawn_opens_one_terminal_in_its_folder_and_area_without_agent_variables() {
    use daifuku_core::config::Area;
    use daifuku_core::grid::{Gaps, Grid};

    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    daifuku_win::dpi::per_monitor_v2();
    let dir = scratch().join("spawn");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let config = peer_config(&dir);
    std::fs::write(
        dir.join("peer-event.json"),
        r#"{"session_id":"e2e-spawn-1","hook_event_name":"PermissionRequest"}"#,
    )
    .unwrap();
    let mut steps = 0;
    // The daemon carries an agent's variables, as one started from inside an
    // agent would: none of them may reach the terminal.
    // Its PATH is not the one in the registry, and it carries a variable of
    // its own: the terminal must see neither, only what Windows builds now.
    let (mut cleanup, _) = start_with(
        &config,
        None,
        &[
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "e2e"),
            ("DAIFUKU_E2E_STALE", "1"),
            ("PATH", r"C:\daifuku-e2e-stale"),
        ],
    );

    let w = spawn(&[
        "--dir",
        &work.display().to_string(),
        "--label",
        "e2e peer",
        "--monitor",
        "primary",
        "--area",
        "right",
    ]);
    let first = ran_full(&dir, "");
    assert!(
        same_dir(&first.folder, &work),
        "the peer started in {}, not in its folder",
        first.folder
    );
    assert_eq!(first.args, "", "a peer without --resume got arguments");
    steps += 1;
    assert_eq!(first.claude, "0", "the terminal saw CLAUDE variables");
    assert_eq!(
        first.stale, "0",
        "the terminal saw the daemon's own variable"
    );
    assert_eq!(first.fresh_path, "1", "the terminal got the daemon's PATH");
    steps += 1;

    // In the right half of the primary monitor, as a fleet of one.
    let monitors = daifuku_win::monitor::monitors();
    let primary = monitors.iter().find(|m| m.primary).unwrap();
    let cell = Grid::plan(1, Area::Right.within(primary.work), Gaps::default(), None)[0];
    assert_in_cell(w, cell, "the peer in the right half");
    steps += 1;

    // Its agent reported from inside it: waiting, labelled, with its session.
    let agent = wait_for("the peer's agent to wait", Duration::from_secs(20), || {
        status()?
            .agents
            .into_iter()
            .find(|a| a.window == w && a.state.name() == "waiting")
    });
    assert_eq!(agent.label.as_deref(), Some("e2e peer"));
    assert_eq!(agent.session_id.as_deref(), Some("e2e-spawn-1"));
    let peer = status()
        .unwrap()
        .peers
        .into_iter()
        .find(|p| p.window == w)
        .expect("the peer in the status");
    assert!(same_dir(&peer.directory, &work), "{}", peer.directory);
    assert_eq!(peer.session_id.as_deref(), Some("e2e-spawn-1"));
    steps += 1;

    // `wait` sees the state, by label in any case.
    let waited = json(&run(&[
        "wait",
        "E2E PEER",
        "--until",
        "waiting",
        "--timeout",
        "20",
        "--json",
    ]));
    assert_eq!(waited["state"], "waiting", "{waited}");
    assert_eq!(waited["window"], w, "{waited}");
    steps += 1;

    // `label` renames it.
    let labelled = run(&["label", &w.to_string(), "renamed"]);
    assert!(labelled.trim().is_empty(), "{labelled}");
    let agent = status()
        .unwrap()
        .agents
        .into_iter()
        .find(|a| a.window == w)
        .unwrap();
    assert_eq!(agent.label.as_deref(), Some("renamed"));
    steps += 1;

    // A border around it, gone with the window.
    let border = wait_for("a border around the peer", Duration::from_secs(5), || {
        border_around(w)
    });
    assert!(window::close(w));
    wait_for("the peer to close", Duration::from_secs(15), || {
        (!window::exists(w)).then_some(())
    });
    wait_for("the border to go", Duration::from_secs(5), || {
        (!window::is_shown(border)).then_some(())
    });
    wait_for(
        "the peer to leave the status",
        Duration::from_secs(5),
        || status().filter(|s| s.peers.is_empty() && s.agents.iter().all(|a| a.window != w)),
    );
    steps += 1;

    // A peer at a priority runs in a group: its shell runs at it, and the
    // shell's process tree ends with the terminal.
    let low = dir.join("low");
    std::fs::create_dir_all(&low).unwrap();
    std::fs::remove_file(dir.join("ran-.txt")).unwrap();
    std::fs::write(dir.join("ping-me"), "").unwrap();
    let grouped = spawn(&[
        "--dir",
        &low.display().to_string(),
        "--label",
        "low",
        "--priority",
        "below-normal",
    ]);
    let grouped_ran = ran_full(&dir, "");
    std::fs::remove_file(dir.join("ping-me")).unwrap();
    let (shell, ping) = (grouped_ran.pid, grouped_ran.ping);
    assert_eq!(grouped_ran.class, "BelowNormal", "the grouped peer's shell");
    assert!(
        ping != 0 && alive(ping),
        "the grouped peer's ping did not start"
    );
    assert!(window::close(grouped));
    // A ping started hidden, in a console of its own, lives on after its
    // terminal unless the group ends it.
    let start = Instant::now();
    while (alive(shell) || alive(ping)) && start.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !alive(shell),
        "the grouped peer's shell outlived its terminal"
    );
    assert!(
        !alive(ping),
        "what the grouped peer started outlived its terminal"
    );
    steps += 1;

    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

#[test]
fn a_spawned_peer_comes_back_by_its_session_after_a_restart() {
    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    let dir = scratch().join("restore");
    let work = dir.join("work");
    let state = dir.join("state");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let config = peer_config(&dir);
    let mut steps = 0;
    let (mut cleanup, _) = start_with(&config, Some(&state), &[]);

    // A peer closed while the daemon runs leaves the file, so no restore
    // opens it again.
    std::fs::write(
        dir.join("peer-event.json"),
        r#"{"session_id":"e2e-closed-1","hook_event_name":"Stop"}"#,
    )
    .unwrap();
    let closed = spawn(&["--dir", &work.display().to_string(), "--label", "closed"]);
    wait_for("the closed peer's session", Duration::from_secs(30), || {
        status()?
            .peers
            .into_iter()
            .find(|p| p.window == closed && p.session_id.as_deref() == Some("e2e-closed-1"))
    });
    assert!(window::close(closed));
    wait_for(
        "the closed peer to leave the status",
        Duration::from_secs(15),
        || status().filter(|s| s.peers.iter().all(|p| p.window != closed)),
    );
    let kept = std::fs::read_to_string(peers_file(&state)).unwrap();
    assert!(!kept.contains("e2e-closed-1"), "{kept}");
    steps += 1;

    std::fs::write(
        dir.join("peer-event.json"),
        r#"{"session_id":"e2e-restore-1","hook_event_name":"Stop"}"#,
    )
    .unwrap();

    let w = spawn(&["--dir", &work.display().to_string(), "--label", "keep"]);
    wait_for("the peer's session", Duration::from_secs(30), || {
        status()?
            .peers
            .into_iter()
            .find(|p| p.window == w && p.session_id.as_deref() == Some("e2e-restore-1"))
    });
    let kept = std::fs::read_to_string(peers_file(&state)).unwrap();
    assert!(kept.contains("e2e-restore-1"), "{kept}");
    steps += 1;

    // A daemon that restarts while the peer is open, as after an install,
    // finds it open again.
    stop(&mut cleanup);
    let (mut cleanup, back) = start_with(&config, Some(&state), &[]);
    assert!(
        back.peers
            .iter()
            .any(|p| p.window == w && p.label.as_deref() == Some("keep")),
        "{:?}",
        back.peers
    );
    steps += 1;

    // As at a restart of Windows: the daemon stops, then its terminal goes,
    // and the next daemon runs in a later start of Windows. A runner cannot
    // restart, so the file is moved a day back instead.
    stop(&mut cleanup);
    assert!(window::close(w));
    wait_for("the peer to close", Duration::from_secs(15), || {
        (!window::exists(w)).then_some(())
    });
    let file = peers_file(&state);
    let mut kept = json(&std::fs::read_to_string(&file).unwrap());
    let day = 86_400;
    kept["boot"] = (kept["boot"].as_u64().unwrap() - day).into();
    for p in kept["peers"].as_array_mut().unwrap() {
        p["boot"] = (p["boot"].as_u64().unwrap() - day).into();
    }
    std::fs::write(&file, kept.to_string()).unwrap();
    let (mut cleanup, first) = start_with(&config, Some(&state), &[]);
    assert!(first.peers.is_empty(), "{:?}", first.peers);
    let listed = json(&run(&["restore", "--dry-run", "--json"]));
    assert_eq!(
        listed["peers"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    assert_eq!(
        listed["peers"][0]["session_id"], "e2e-restore-1",
        "{listed}"
    );
    assert!(listed["peers"][0]["window"].is_null(), "{listed}");
    steps += 1;

    let restored = json(&run(&["restore", "--json"]));
    let again = restored["peers"][0]["window"]
        .as_u64()
        .unwrap_or_else(|| panic!("restore opened nothing: {restored}"));
    let (folder, args, _, _) = ran(&dir, "--resume-e2e-restore-1");
    assert!(same_dir(&folder, &work), "{folder}");
    assert_eq!(args, "--resume e2e-restore-1");
    steps += 1;

    // Nothing is left to open, and the reopened peer is open again.
    let listed = json(&run(&["restore", "--dry-run", "--json"]));
    assert_eq!(listed["peers"], serde_json::json!([]), "{listed}");
    assert!(status().unwrap().peers.iter().any(|p| p.window == again));
    steps += 1;

    let _ = window::close(again);
    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

#[test]
fn a_lead_on_the_left_and_a_crew_of_three_on_the_right_open_on_one_key() {
    use daifuku_core::config::Area;
    use daifuku_core::grid::{Gaps, Grid, Shape};

    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    daifuku_win::dpi::per_monitor_v2();
    let dir = scratch().join("layout");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("daifuku.json");
    let fleets = serde_json::json!({
        "fleets": [
            { "name": "lead", "count": 1, "monitor": "primary", "area": "left",
              "command": null, "no_profile": true, "hotkey": null, "with": ["crew"] },
            { "name": "crew", "count": 3, "monitor": "primary", "area": "right",
              "shape": { "columns": 1, "rows": 3 }, "command": null, "no_profile": true,
              "hotkey": null }
        ],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, fleets.to_string()).unwrap();
    let mut steps = 0;
    let (mut cleanup, _) = start_with(&config, None, &[]);

    let opened = run(&["open", "lead"]);
    assert!(
        opened.contains("opened crew (3 terminals") && opened.contains("opened lead (1 terminal"),
        "{opened}"
    );
    let s = wait_for("both fleets in the status", Duration::from_secs(10), || {
        status().filter(|s| s.fleets.len() == 2)
    });
    let windows = |name: &str| {
        s.fleets
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.windows.clone())
            .unwrap()
    };
    let (lead, crew) = (windows("lead"), windows("crew"));
    assert_eq!((lead.len(), crew.len()), (1, 3));
    steps += 1;

    let monitors = daifuku_win::monitor::monitors();
    let work = monitors.iter().find(|m| m.primary).unwrap().work;
    let gaps = Gaps::default();
    let left = Grid::plan(1, Area::Left.within(work), gaps, None);
    let right = Grid::plan(
        3,
        Area::Right.within(work),
        gaps,
        Some(Shape {
            columns: 1,
            rows: 3,
        }),
    );
    assert_in_cell(lead[0], left[0], "the lead");
    for (i, &w) in crew.iter().enumerate() {
        assert_in_cell(w, right[i], &format!("crew terminal {}", i + 1));
    }
    // Three rows the width of the half, not two over one.
    assert!(
        right
            .iter()
            .all(|c| c.left == right[0].left && c.right == right[0].right)
    );
    steps += 1;

    // The lead opened last, so it should be in front.
    expect_failure(
        "the lead has the focus after one key opened both fleets",
        window::foreground() == lead[0],
    );

    let closed = run(&["close", "--all"]);
    assert!(closed.contains("closed 4 terminals"), "{closed}");
    steps += 1;

    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

#[test]
fn a_border_goes_when_its_agent_process_is_killed() {
    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    let dir = scratch().join("kill");
    std::fs::create_dir_all(&dir).unwrap();
    let config = peer_config(&dir);
    std::fs::write(
        dir.join("peer-event.json"),
        r#"{"session_id":"e2e-kill-1","hook_event_name":"PermissionRequest"}"#,
    )
    .unwrap();
    let mut steps = 0;
    let (mut cleanup, _) = start_with(&config, None, &[]);

    // A console of its own whose processes are killed, not closed.
    let tag = format!("daifuku-e2e-kill-{}", std::process::id());
    let script = format!(
        "title {tag} & \"{h}\" hook < \"{e}\" & ping -n 120 127.0.0.1 >nul",
        h = cli().display(),
        e = dir.join("peer-event.json").display(),
    );
    let mut console = Command::new("cmd.exe")
        .args(["/d", "/s", "/c"])
        .raw_arg(format!("\"{script}\""))
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .unwrap();
    let agent = wait_for(
        "the console's agent to wait",
        Duration::from_secs(15),
        || {
            status()?
                .agents
                .into_iter()
                .find(|a| a.state.name() == "waiting" && a.title.contains(&tag))
        },
    );
    let border = wait_for(
        "a border around the console",
        Duration::from_secs(5),
        || border_around(agent.window),
    );
    // The whole tree, as a crash takes it: cmd and the ping it started.
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &console.id().to_string()])
        .output();
    let _ = console.wait();
    wait_for(
        "the border to go with the killed console",
        Duration::from_secs(15),
        || (!window::is_shown(border)).then_some(()),
    );
    let _ = window::close(agent.window);
    steps += 1;

    // A Windows Terminal tab whose shell is killed: the tab stays open,
    // showing that the process ended, and the agent's process is all that
    // says the agent is gone.
    if daifuku_win::terminal::find().is_some() {
        let w = spawn(&["--dir", &dir.display().to_string(), "--label", "killed"]);
        let (_, _, _, pid) = ran(&dir, "");
        let border = wait_for("a border around the tab", Duration::from_secs(20), || {
            status()?
                .agents
                .iter()
                .any(|a| a.window == w && a.state.name() == "waiting")
                .then(|| border_around(w))
                .flatten()
        });
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
        wait_for(
            "the border to go with the killed shell",
            Duration::from_secs(15),
            || (!window::is_shown(border)).then_some(()),
        );
        wait_for("the agent to be forgotten", Duration::from_secs(5), || {
            status().filter(|s| s.agents.iter().all(|a| a.window != w))
        });
        steps += 1;
        let _ = window::close(w);
        wait_for("the tab to close", Duration::from_secs(15), || {
            (!window::exists(w)).then_some(())
        });

        // The case as it happens: the agent ends, and the shell it ran in
        // stays, its prompt back. The agent here is a second PowerShell on
        // the tab's console that reports two seconds after the shell started.
        std::fs::remove_file(dir.join("ran-.txt")).unwrap();
        std::fs::write(dir.join("agent-me"), "").unwrap();
        let w = spawn(&["--dir", &dir.display().to_string(), "--label", "agent"]);
        let shell = ran_full(&dir, "").pid;
        let agent: u32 = wait_for("the agent's process id", Duration::from_secs(20), || {
            std::fs::read_to_string(dir.join("agent-pid.txt"))
                .ok()
                .and_then(|t| t.trim().parse().ok())
        });
        let border = wait_for(
            "a border around the agent's tab",
            Duration::from_secs(20),
            || {
                status()?
                    .agents
                    .iter()
                    .any(|a| a.window == w && a.state.name() == "waiting")
                    .then(|| border_around(w))
                    .flatten()
            },
        );
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &agent.to_string()])
            .output();
        wait_for(
            "the border to go with the agent",
            Duration::from_secs(15),
            || (!window::is_shown(border)).then_some(()),
        );
        assert!(
            alive(shell) && window::exists(w),
            "the shell and its tab stay"
        );
        wait_for("the agent to be forgotten", Duration::from_secs(5), || {
            status().filter(|s| s.agents.iter().all(|a| a.window != w))
        });
        steps += 1;
        let _ = window::close(w);
    }

    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

/// `daifuku spawn` with `args`, as the daemon answered it.
fn spawned(args: &[&str]) -> (u64, Option<String>, Option<u32>) {
    let mut all = vec!["spawn"];
    all.extend_from_slice(args);
    all.push("--json");
    let reply = run(&all);
    match from_line::<Response>(&reply) {
        Ok(Response::Spawned {
            window,
            fleet,
            cell,
            ..
        }) => (window, fleet, cell),
        other => panic!("spawn answered {other:?}: {reply}"),
    }
}

/// The open fleet `name`'s terminals, in cell order.
fn fleet_windows(name: &str) -> Vec<u64> {
    status()
        .unwrap()
        .fleets
        .into_iter()
        .find(|f| f.name == name)
        .map(|f| f.windows)
        .unwrap_or_default()
}

#[test]
fn peers_take_the_cells_of_a_crew_close_one_at_a_time_and_come_back_to_them() {
    use daifuku_core::config::Area;
    use daifuku_core::grid::{Gaps, Grid, Shape};
    use daifuku_core::protocol::RestoredPeer;

    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    daifuku_win::dpi::per_monitor_v2();
    let dir = scratch().join("crew");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    // A crew of two, stacked on the right half: each terminal runs the peer
    // script with its number, which names the file it writes and the event
    // it reports, so each one has a session of its own.
    let config = dir.join("daifuku.json");
    let command = format!("& '{}' {{n}}", peer_script(&dir).display());
    let json = serde_json::json!({
        "fleets": [{
            "name": "crew", "count": 2, "monitor": "primary", "area": "right",
            "shape": { "columns": 1, "rows": 2 },
            "command": command, "admin": true, "no_profile": true, "hotkey": null
        }],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, json.to_string()).unwrap();
    let events = |on: bool| {
        for n in 1..=2 {
            let file = dir.join(format!("peer-event-{n}.json"));
            if on {
                let event = format!(r#"{{"session_id":"e2e-crew-{n}","hook_event_name":"Stop"}}"#);
                std::fs::write(&file, event).unwrap();
            } else {
                let _ = std::fs::remove_file(&file);
            }
        }
    };
    events(true);
    let mut steps = 0;
    let (mut cleanup, _) = start_with(&config, None, &[]);
    let monitors = daifuku_win::monitor::monitors();
    let primary = monitors.iter().find(|m| m.primary).unwrap();
    let cells = Grid::plan(
        2,
        Area::Right.within(primary.work),
        Gaps::default(),
        Some(Shape {
            columns: 1,
            rows: 2,
        }),
    );
    let dir_arg = work.display().to_string();
    let at = |args: &[&str]| {
        let mut all = vec!["--dir", dir_arg.as_str(), "--fleet", "crew"];
        all.extend_from_slice(args);
        spawned(&all)
    };
    // A peer's session once its agent reported it from inside it.
    let session_of = |w: u64| {
        wait_for("a peer's session", Duration::from_secs(20), || {
            status()?
                .peers
                .into_iter()
                .find(|p| p.window == w)
                .and_then(|p| p.session_id)
        })
    };
    let restore = |args: &[&str]| -> Vec<RestoredPeer> {
        let mut all = vec!["restore"];
        all.extend_from_slice(args);
        all.push("--json");
        let reply = run(&all);
        match from_line::<Response>(&reply) {
            Ok(Response::Restored { peers }) => peers,
            other => panic!("restore answered {other:?}: {reply}"),
        }
    };
    let terminals = || {
        window::top_level()
            .into_iter()
            .filter(|&w| window::class(w) == daifuku_win::terminal::WINDOW_CLASS)
            .count()
    };

    // The first peer takes the first cell and is the crew's first terminal,
    // numbered as it.
    let (a, fleet, cell) = at(&["--cell", "--label", "site"]);
    assert_eq!((fleet.as_deref(), cell), (Some("crew"), Some(1)));
    let first = ran_full(&dir, "1");
    assert!(same_dir(&first.folder, &work), "{}", first.folder);
    assert_in_cell(a, cells[0], "the first peer");
    assert_eq!(session_of(a), "e2e-crew-1");
    steps += 1;

    // The second takes the next one; the crew is open with both.
    let (b, _, cell) = at(&["--cell"]);
    assert_eq!(cell, Some(2));
    assert_in_cell(b, cells[1], "the second peer");
    assert_eq!(session_of(b), "e2e-crew-2");
    assert_eq!(fleet_windows("crew"), vec![a, b]);
    steps += 1;

    // A third finds no cell and opens nothing.
    let before = terminals();
    let full = run(&[
        "spawn", "--dir", &dir_arg, "--fleet", "crew", "--cell", "--json",
    ]);
    assert!(full.contains("every cell of fleet `crew`"), "{full}");
    assert_eq!(fleet_windows("crew"), vec![a, b]);
    assert_eq!(terminals(), before, "a third terminal opened");
    steps += 1;

    // Snap puts a peer that was moved back in its cell, as any of the crew.
    let _ = window::place(a, daifuku_core::Rect::new(10, 10, 600, 400));
    run(&["snap"]);
    assert_in_cell(a, cells[0], "the first peer after a snap");
    steps += 1;

    // Close one by its label: only it goes, its cell is free again, and as
    // it was closed by hand, nothing waits to be restored.
    let closed = run(&["close", "--terminal", "SITE"]);
    assert!(closed.contains("closed site"), "{closed}");
    assert!(!window::exists(a), "the peer named is still open");
    assert!(window::exists(b), "the other peer closed too");
    wait_for("the crew to keep one", Duration::from_secs(5), || {
        (fleet_windows("crew") == vec![b]).then_some(())
    });
    assert!(restore(&["--dry-run"]).is_empty());
    let unknown = run(&["close", "--terminal", "nobody"]);
    assert!(
        unknown.contains("no terminal is called `nobody`"),
        "{unknown}"
    );
    steps += 1;

    // A peer that asks for the first cell gets it back.
    std::fs::remove_file(dir.join("ran-1.txt")).unwrap();
    let (c, _, cell) = at(&["--cell", "1", "--label", "site"]);
    assert_eq!(cell, Some(1));
    ran_full(&dir, "1");
    assert_in_cell(c, cells[0], "the peer in the freed cell");
    assert_eq!(fleet_windows("crew"), vec![c, b]);
    assert_eq!(session_of(c), "e2e-crew-1");
    steps += 1;

    // Closed with their fleet, as the close key does: kept for restore,
    // which opens each again in its own cell.
    run(&["close", "crew"]);
    wait_for("the crew to close", Duration::from_secs(15), || {
        (!window::exists(b) && !window::exists(c)).then_some(())
    });
    let mut kept: Vec<String> = restore(&["--dry-run"])
        .into_iter()
        .map(|p| p.session_id)
        .collect();
    kept.sort();
    assert_eq!(kept, ["e2e-crew-1", "e2e-crew-2"]);
    let back = restore(&[]);
    assert_eq!(back.len(), 2, "{back:?}");
    for p in &back {
        assert_eq!(p.error, None, "{p:?}");
        let w = p.window.expect("a restored window");
        let cell = if p.session_id == "e2e-crew-1" { 0 } else { 1 };
        assert_in_cell(w, cells[cell], &format!("{} restored", p.session_id));
        assert!(
            fleet_windows("crew").contains(&w),
            "{p:?} is not in the crew"
        );
    }
    steps += 1;

    // Closed with the crew again, then the crew opened afresh, which fills
    // both cells: restore still opens them, beside the crew.
    run(&["close", "crew"]);
    wait_for("the crew to close again", Duration::from_secs(15), || {
        status().filter(|s| s.fleets.is_empty() && s.peers.is_empty())
    });
    events(false);
    run(&["open", "crew"]);
    wait_for(
        "the crew to fill its cells",
        Duration::from_secs(30),
        || (fleet_windows("crew").len() == 2).then_some(()),
    );
    let crew = fleet_windows("crew");
    let beside = restore(&[]);
    assert_eq!(beside.len(), 2, "{beside:?}");
    for p in &beside {
        assert_eq!(p.error, None, "{p:?}");
        let w = p.window.expect("a restored window");
        assert!(window::exists(w));
        assert!(!crew.contains(&w), "{p:?} took a cell of the crew");
    }
    assert_eq!(fleet_windows("crew"), crew, "the crew kept its terminals");
    steps += 1;

    // By its window, a peer beside the crew; then the rest with the crew.
    for p in &beside {
        let w = p.window.unwrap();
        let closed = run(&["close", "--terminal", &format!("{w:#x}")]);
        assert!(closed.contains("closed"), "{closed}");
        assert!(!window::exists(w));
    }
    run(&["close", "crew"]);
    wait_for("nothing to be left", Duration::from_secs(15), || {
        status().filter(|s| s.peers.is_empty() && s.fleets.is_empty())
    });
    steps += 1;

    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

#[test]
fn a_peer_in_a_cell_stays_in_its_fleet_when_the_daemon_was_gone_for_long() {
    use daifuku_core::config::Area;
    use daifuku_core::grid::{Gaps, Grid, Shape};
    use daifuku_core::saved::FRESH_SECONDS;

    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    daifuku_win::dpi::per_monitor_v2();
    let dir = scratch().join("rejoin");
    let work = dir.join("work");
    let state = dir.join("state");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    // A crew of two stacked on the right half, each terminal with a session
    // of its own, as in the crew test above.
    let config = dir.join("daifuku.json");
    let command = format!("& '{}' {{n}}", peer_script(&dir).display());
    let fleets = serde_json::json!({
        "fleets": [{
            "name": "crew", "count": 2, "monitor": "primary", "area": "right",
            "shape": { "columns": 1, "rows": 2 },
            "command": command, "admin": true, "no_profile": true, "hotkey": null
        }],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, fleets.to_string()).unwrap();
    for n in 1..=2 {
        let event = format!(r#"{{"session_id":"e2e-rejoin-{n}","hook_event_name":"Stop"}}"#);
        std::fs::write(dir.join(format!("peer-event-{n}.json")), event).unwrap();
    }
    let monitors = daifuku_win::monitor::monitors();
    let primary = monitors.iter().find(|m| m.primary).unwrap();
    let cells = Grid::plan(
        2,
        Area::Right.within(primary.work),
        Gaps::default(),
        Some(Shape {
            columns: 1,
            rows: 2,
        }),
    );
    let terminals = || {
        window::top_level()
            .into_iter()
            .filter(|&w| window::class(w) == daifuku_win::terminal::WINDOW_CLASS)
            .count()
    };
    let mut steps = 0;
    let (mut cleanup, _) = start_with(&config, Some(&state), &[]);

    // A peer takes the crew's first cell and the crew's own terminal the
    // second; the peers file says which cell of which fleet the peer is in.
    let dir_arg = work.display().to_string();
    let (a, fleet, cell) = spawned(&["--dir", &dir_arg, "--fleet", "crew", "--cell"]);
    assert_eq!((fleet.as_deref(), cell), (Some("crew"), Some(1)));
    ran_full(&dir, "1");
    wait_for("the peer's session", Duration::from_secs(20), || {
        status()?
            .peers
            .into_iter()
            .find(|p| p.window == a && p.session_id.as_deref() == Some("e2e-rejoin-1"))
    });
    run(&["open", "crew"]);
    let own = wait_for("the crew's own terminal", Duration::from_secs(30), || {
        let crew = fleet_windows("crew");
        (crew.len() == 2 && crew[0] == a).then(|| crew[1])
    });
    let kept = json(&std::fs::read_to_string(peers_file(&state)).unwrap());
    assert_eq!(
        kept["peers"][0]["in_cell"],
        serde_json::json!({ "fleet": "crew", "cell": 1 }),
        "{kept}"
    );
    steps += 1;

    // The daemon stops for longer than its save of the fleets stays fresh.
    // A runner cannot be made to wait that long, so the save is moved back
    // that far instead, once Windows has been up for longer than that.
    stop(&mut cleanup);
    let ticks_per_second = 10_000_000;
    let stale = (FRESH_SECONDS + 60) * ticks_per_second;
    wait_for(
        "Windows to be up for longer than a save stays fresh",
        Duration::from_secs(FRESH_SECONDS + 120),
        || (daifuku_win::clock::ticks() > stale).then_some(()),
    );
    let file = state_file(&state);
    let mut saved = json(&std::fs::read_to_string(&file).unwrap());
    saved["at"] = saved["at"].as_u64().unwrap().saturating_sub(stale).into();
    std::fs::write(&file, saved.to_string()).unwrap();
    let (mut cleanup, back) = start_with(&config, Some(&state), &[]);
    // The save is not taken back, so the crew's own terminal is let go; the
    // peer is open again, and in its cell of the crew.
    assert!(back.peers.iter().any(|p| p.window == a), "{:?}", back.peers);
    let crew: Vec<u64> = back
        .fleets
        .iter()
        .find(|f| f.name == "crew")
        .map(|f| f.windows.clone())
        .unwrap_or_default();
    assert_eq!(crew, vec![a], "{:?}", back.fleets);
    assert!(window::exists(own), "the crew's own terminal closed");
    steps += 1;

    // The crew's key opens its free cell only, no terminal over the peer.
    let before = terminals();
    run(&["open", "crew"]);
    let crew = wait_for(
        "the crew to fill its cells",
        Duration::from_secs(30),
        || {
            let crew = fleet_windows("crew");
            (crew.len() == 2).then_some(crew)
        },
    );
    assert_eq!(crew[0], a, "{crew:?}");
    assert!(!crew.contains(&own), "{crew:?}");
    assert_eq!(terminals(), before + 1, "a terminal opened over the peer");
    assert_in_cell(a, cells[0], "the peer after its crew opened");
    steps += 1;

    // Closing the crew closes the peer with it, kept for restore.
    run(&["close", "crew"]);
    wait_for("the crew to close", Duration::from_secs(15), || {
        (!window::exists(a) && !window::exists(crew[1])).then_some(())
    });
    let listed = json(&run(&["restore", "--dry-run", "--json"]));
    assert_eq!(listed["peers"][0]["session_id"], "e2e-rejoin-1", "{listed}");
    steps += 1;

    let _ = window::close(own);
    wait_for(
        "the terminal let go to close",
        Duration::from_secs(15),
        || (!window::exists(own)).then_some(()),
    );
    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

#[test]
fn stop_agents_closes_every_agent_ends_what_it_left_and_restore_brings_it_back() {
    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    let dir = scratch().join("stop-agents");
    let work = dir.join("work");
    let state = dir.join("state");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let config = peer_config(&dir);
    // The agent reports a folder inside the one the fleet's terminal starts
    // in, and starts a ping that outlives its terminal.
    let event = serde_json::json!({
        "session_id": "e2e-stop-1",
        "hook_event_name": "Stop",
        "cwd": work.display().to_string(),
    });
    std::fs::write(dir.join("peer-event.json"), event.to_string()).unwrap();
    std::fs::write(dir.join("agent-me"), "").unwrap();
    std::fs::write(dir.join("leftover-me"), "").unwrap();
    let mut steps = 0;
    let (mut cleanup, _) = start_with(&config, Some(&state), &[]);

    run(&["open", "e2e"]);
    let w = wait_for("the fleet's terminal", Duration::from_secs(30), || {
        fleet_windows("e2e").first().copied()
    });
    let leftover: u32 = wait_for("the agent's ping", Duration::from_secs(30), || {
        std::fs::read_to_string(dir.join("leftover-pid.txt"))
            .ok()
            .and_then(|t| t.trim().parse().ok())
    });
    wait_for("the agent's session", Duration::from_secs(30), || {
        status()?.agents.into_iter().find(|a| {
            a.window == w && a.session_id.as_deref() == Some("e2e-stop-1") && a.cwd.is_some()
        })
    });
    steps += 1;

    // A dry run says what it would do and does none of it.
    let planned = json(&run(&["stop-agents", "--dry-run", "--json"]));
    println!("e2e: stop-agents --dry-run --json: {planned}");
    assert_eq!(planned["dry_run"], true, "{planned}");
    let terminal = &planned["terminals"][0];
    assert_eq!(terminal["window"], w, "{planned}");
    assert_eq!(terminal["fleet"], "e2e", "{planned}");
    assert_eq!(terminal["cell"], 1, "{planned}");
    assert_eq!(terminal["session_id"], "e2e-stop-1", "{planned}");
    assert_eq!(terminal["kept"], true, "{planned}");
    assert_eq!(terminal["closed"], false, "{planned}");
    let listed = |reply: &serde_json::Value| {
        reply["leftovers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|l| l["pid"] == leftover)
            .cloned()
    };
    assert!(listed(&planned).is_some(), "{planned}");
    assert!(window::exists(w) && alive(leftover));
    steps += 1;

    let stopped = json(&run(&["stop-agents", "--json"]));
    println!("e2e: stop-agents --json: {stopped}");
    assert_eq!(stopped["terminals"][0]["closed"], true, "{stopped}");
    assert_eq!(stopped["terminals"][0]["kept"], true, "{stopped}");
    let ping = listed(&stopped).unwrap_or_else(|| panic!("the ping is not listed: {stopped}"));
    assert_eq!(ping["ended"], true, "{stopped}");
    assert!(!window::exists(w));
    wait_for("the ping to end", Duration::from_secs(5), || {
        (!alive(leftover)).then_some(())
    });
    assert!(fleet_windows("e2e").is_empty());
    steps += 1;

    // Restore opens it again in its cell, on its session, in its folder.
    let listed = json(&run(&["restore", "--dry-run", "--json"]));
    assert_eq!(listed["peers"][0]["session_id"], "e2e-stop-1", "{listed}");
    std::fs::remove_file(dir.join("leftover-me")).unwrap();
    std::fs::remove_file(dir.join("agent-me")).unwrap();
    let restored = json(&run(&["restore", "--json"]));
    let again = restored["peers"][0]["window"]
        .as_u64()
        .unwrap_or_else(|| panic!("restore opened nothing: {restored}"));
    let (folder, args, _, _) = ran(&dir, "--resume-e2e-stop-1");
    assert!(same_dir(&folder, &work), "{folder}");
    assert_eq!(args, "--resume e2e-stop-1");
    assert_eq!(fleet_windows("e2e"), vec![again]);
    steps += 1;

    // Opened again, it is a peer, and the next stop keeps it the same way.
    let stopped = json(&run(&["stop-agents", "--json"]));
    assert_eq!(stopped["terminals"][0]["window"], again, "{stopped}");
    assert_eq!(stopped["terminals"][0]["closed"], true, "{stopped}");
    assert_eq!(stopped["terminals"][0]["kept"], true, "{stopped}");
    let listed = json(&run(&["restore", "--dry-run", "--json"]));
    assert_eq!(listed["peers"][0]["session_id"], "e2e-stop-1", "{listed}");
    steps += 1;

    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

#[test]
fn a_fleet_that_resumes_opens_each_terminal_back_in_its_cell_and_session() {
    use daifuku_core::config::Area;
    use daifuku_core::grid::{Gaps, Grid, Shape};

    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    daifuku_win::dpi::per_monitor_v2();
    let dir = scratch().join("resume");
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    // A crew of two that resumes, stacked on the right half, and a fleet of
    // one beside it that does not. Each terminal runs the peer script with
    // its number, which reports a session of its own from inside it.
    let config = dir.join("daifuku.json");
    let script = peer_script(&dir);
    let fleets = serde_json::json!({
        "fleets": [{
            "name": "crew", "count": 2, "monitor": "primary", "area": "right",
            "shape": { "columns": 1, "rows": 2 }, "resume": true,
            "command": format!("& '{}' {{n}}", script.display()),
            "admin": true, "no_profile": true, "hotkey": null
        }, {
            "name": "plain", "count": 1, "monitor": "primary", "area": "left",
            "command": format!("& '{}' plain", script.display()),
            "admin": true, "no_profile": true, "hotkey": null
        }],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, fleets.to_string()).unwrap();
    for name in ["1", "2", "plain"] {
        let event = format!(r#"{{"session_id":"e2e-resume-{name}","hook_event_name":"Stop"}}"#);
        std::fs::write(dir.join(format!("peer-event-{name}.json")), event).unwrap();
    }
    let primary = daifuku_win::monitor::monitors()
        .into_iter()
        .find(|m| m.primary)
        .unwrap();
    let cells = Grid::plan(
        2,
        Area::Right.within(primary.work),
        Gaps::default(),
        Some(Shape {
            columns: 1,
            rows: 2,
        }),
    );
    let sessions = || -> Vec<(u64, String)> {
        status()
            .unwrap()
            .peers
            .into_iter()
            .filter_map(|p| Some((p.window, p.session_id?)))
            .collect()
    };
    let kept = || {
        let listed = json(&run(&["restore", "--dry-run", "--json"]));
        let mut ids: Vec<String> = listed["peers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["session_id"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    };
    // Both crew terminals open again, each in its cell, resuming the
    // session it ran, and no new one starts beside them.
    let resumed = |what: &str| {
        let fresh = dir.join("ran-1.txt");
        let _ = std::fs::remove_file(&fresh);
        for n in 1..=2 {
            let _ = std::fs::remove_file(dir.join(format!("ran-{n}---resume-e2e-resume-{n}.txt")));
        }
        let _ = run(&["open", "crew"]);
        for n in 1..=2 {
            let r = ran_full(&dir, &format!("{n}---resume-e2e-resume-{n}"));
            assert_eq!(r.args, format!("{n} --resume e2e-resume-{n}"), "{what}");
        }
        let windows = fleet_windows("crew");
        assert_eq!(windows.len(), 2, "{what}: {windows:?}");
        for (i, &w) in windows.iter().enumerate() {
            assert_in_cell(w, cells[i], &format!("{what}: terminal {}", i + 1));
        }
        assert!(!fresh.exists(), "{what}: a fresh terminal opened too");
        windows
    };
    let mut steps = 0;
    let (mut cleanup, _) = start_with(&config, Some(&state), &[]);

    // Opened afresh, each crew terminal is kept with the session its agent
    // reports from inside it, as a peer `daifuku spawn` opened would be.
    let _ = run(&["open", "crew"]);
    ran_full(&dir, "1");
    ran_full(&dir, "2");
    let crew = fleet_windows("crew");
    assert_eq!(crew.len(), 2, "{crew:?}");
    wait_for("both crew sessions", Duration::from_secs(20), || {
        let s = sessions();
        crew.iter()
            .enumerate()
            .all(|(i, w)| s.contains(&(*w, format!("e2e-resume-{}", i + 1))))
            .then_some(())
    });
    let file = std::fs::read_to_string(peers_file(&state)).unwrap();
    assert!(file.contains("e2e-resume-2"), "{file}");
    steps += 1;

    // A fleet that does not resume keeps nothing, once the daemon has its
    // agent's report.
    let _ = run(&["open", "plain"]);
    ran_full(&dir, "plain");
    let plain = fleet_windows("plain");
    assert_eq!(plain.len(), 1);
    wait_for("the plain fleet's agent", Duration::from_secs(20), || {
        status()?
            .agents
            .iter()
            .any(|a| a.window == plain[0])
            .then_some(())
    });
    assert!(
        sessions().iter().all(|(w, _)| *w != plain[0]),
        "{:?}",
        sessions()
    );
    let _ = run(&["close", "plain"]);
    steps += 1;

    // Closed with its fleet, each waits to come back; opened again, each
    // comes back in its cell, resuming.
    let _ = run(&["close", "crew"]);
    assert!(fleet_windows("crew").is_empty());
    assert_eq!(kept(), ["e2e-resume-1", "e2e-resume-2"]);
    let again = resumed("after a close");
    assert!(kept().is_empty(), "{:?}", kept());
    steps += 1;

    // As at a restart of Windows: the daemon stops, then the terminals go,
    // and the next daemon runs in a later start of Windows. A runner cannot
    // restart, so the file is moved a day back instead.
    stop(&mut cleanup);
    for &w in &again {
        assert!(window::close(w));
    }
    wait_for("the crew to close", Duration::from_secs(15), || {
        again.iter().all(|&w| !window::exists(w)).then_some(())
    });
    let file = peers_file(&state);
    let mut saved = json(&std::fs::read_to_string(&file).unwrap());
    let day = 86_400;
    saved["boot"] = (saved["boot"].as_u64().unwrap() - day).into();
    for p in saved["peers"].as_array_mut().unwrap() {
        p["boot"] = (p["boot"].as_u64().unwrap() - day).into();
    }
    std::fs::write(&file, saved.to_string()).unwrap();
    let (mut cleanup, first) = start_with(&config, Some(&state), &[]);
    assert!(first.peers.is_empty(), "{:?}", first.peers);
    assert_eq!(kept(), ["e2e-resume-1", "e2e-resume-2"]);
    let back = resumed("after a restart");
    steps += 1;

    for w in back {
        let _ = window::close(w);
    }
    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}

/// Shift pressed and released: input that wakes the saver and types nothing.
fn press_shift() -> u32 {
    keys(&[(0x10, 0x2A, true), (0x10, 0x2A, false)])
}

#[test]
fn a_fleet_opened_while_nobody_is_at_the_desk_does_not_put_the_saver_off() {
    use daifuku_win::saver;

    if !enabled() {
        eprintln!("skipped: set DAIFUKU_E2E=1 to run against a real desktop");
        return;
    }
    if daifuku_win::terminal::find().is_none() {
        eprintln!("skipped: Windows Terminal is not installed here");
        return;
    }
    // A short saver would come up in the middle of the other tests, so CI
    // sets one for this test alone, in a step of its own.
    if std::env::var_os("DAIFUKU_E2E_SAVER").is_none() {
        eprintln!("skipped: set DAIFUKU_E2E_SAVER=1 once a one-minute screen saver is set");
        return;
    }
    let timeout = saver::Now::read()
        .timeout
        .expect("a screen saver is set and active");
    let dir = scratch().join("saver");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("daifuku.json");
    let json = serde_json::json!({
        "fleets": [{
            "name": "e2e", "count": 1, "command": "Start-Sleep -Seconds 300",
            "admin": true, "no_profile": true, "hotkey": null
        }],
        "hotkeys": { "next_waiting": null, "snap": null, "close": null }
    });
    std::fs::write(&config, json.to_string()).unwrap();
    let mut steps = 0;
    let (mut cleanup, _) = start(&config);

    // The last input, then nothing but the new window a while later: a peer
    // the fleet opens at night. Windows starts the saver's countdown over
    // when that window comes up, and the daemon has to make up for it.
    assert_eq!(press_shift(), 2);
    let quiet = Instant::now();
    std::thread::sleep(timeout * 2 / 5);
    assert!(
        saver::Now::read().idle >= timeout / 3,
        "something else made input while the test waited"
    );
    let opened = run(&["open", "e2e"]);
    assert!(opened.contains("opened e2e (1 terminal"), "{opened}");
    let fleet = wait_for("the terminal", Duration::from_secs(10), || {
        status()?
            .fleets
            .into_iter()
            .find(|f| f.name == "e2e" && f.windows.len() == 1)
    });
    steps += 1;

    // Windows alone starts it a full timeout after the window came up.
    wait_for("the saver", timeout * 2, || {
        saver::Now::read().running.then_some(())
    });
    let late = quiet.elapsed().saturating_sub(timeout);
    assert!(
        late < Duration::from_secs(10),
        "the saver started {late:?} after it was due"
    );
    steps += 1;

    assert_eq!(press_shift(), 2);
    wait_for("the saver to end", Duration::from_secs(10), || {
        (!saver::Now::read().running).then_some(())
    });
    let closed = run(&["close", "e2e"]);
    assert!(closed.contains("closed 1 terminal"), "{closed}");
    wait_for("the terminal to close", Duration::from_secs(15), || {
        (!window::exists(fleet.windows[0])).then_some(())
    });
    steps += 1;

    stop(&mut cleanup);
    let _ = std::fs::remove_dir_all(&dir);
    println!("e2e: {steps} steps passed");
}
