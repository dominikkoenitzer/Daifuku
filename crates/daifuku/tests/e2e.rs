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
            for fleet in ["e2e", "demo"] {
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
        r#"{"fleets":[{"name":"e2e","hotkey":null}],"hotkeys":{"next_waiting":null,"snap":null}}"#,
    )
    .unwrap();
    let mut steps = 0;

    let (mut cleanup, first) = start(&config);
    assert!(first.agents.is_empty() && first.fleets.is_empty());
    steps += 1;

    // A console of its own, like a terminal an agent runs in: the hook
    // reports waiting, working and done, a second apart, and the console then
    // stays open until the test closes it.
    let hook = cli();
    let script = format!(
        "\"{h}\" hook < \"{w}\" & ping -n 3 127.0.0.1 >nul & \"{h}\" hook < \"{k}\" & ping -n 3 127.0.0.1 >nul & \"{h}\" hook < \"{d}\" & ping -n 60 127.0.0.1 >nul",
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
            .find(|a| a.state.name() == "waiting")
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
        "hotkeys": { "next_waiting": null, "snap": null }
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

    let _ = run(&["stop"]);
    let mut daemon = cleanup.0.pop().unwrap();
    wait_for("the daemon to exit", Duration::from_secs(10), || {
        daemon.try_wait().ok().flatten()
    });
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
        r#"{"fleets":[],"hotkeys":{"next_waiting":"ctrl + alt + f11","snap":null}}"#,
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
