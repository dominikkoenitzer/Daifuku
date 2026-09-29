//! End to end, on a real desktop: the real daemon, a real console window, the
//! real hook reporting from inside it, and the border that results.
//!
//! Opt-in, because it starts a daemon and opens a window:
//!
//! ```text
//! cargo build --workspace
//! set DAIFUKU_E2E=1
//! cargo test -p daifuku --test e2e -- --nocapture
//! ```
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

struct Cleanup(Vec<Child>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new(cli()).arg("stop").output();
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

    let daemon = Command::new(daemon_exe())
        .arg("--config")
        .arg(&config)
        .spawn()
        .unwrap();
    let mut cleanup = Cleanup(vec![daemon]);
    let first = wait_for("the daemon to answer", Duration::from_secs(15), status);
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
