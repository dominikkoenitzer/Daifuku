//! `daifuku group` and `daifuku heavy`, run for real: the priority reaches
//! every process in the group, the exit code comes back, what the program
//! leaves running ends with it, and heavy jobs take turns.
//!
//! They start only console programs, without a window, and talk to no
//! daemon, so they run in every `cargo test`.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn cli() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_daifuku"))
}

/// `daifuku`, on a heavy-job queue of this test run's own, so a test never
/// waits for a real build on the machine, nor a build for a test. A heavy
/// job the suite itself runs in holds its turn on the real queue, not on
/// this one, so its mark is not passed on.
fn daifuku() -> Command {
    let mut c = Command::new(cli());
    c.env(
        "DAIFUKU_HEAVY_QUEUE",
        format!(r"Local\Daifuku.Heavy.test-{}", std::process::id()),
    )
    .env_remove("DAIFUKU_HEAVY");
    c
}

/// What a PowerShell line run inside `daifuku <args> --` printed.
fn powershell_in(args: &[&str], line: &str) -> String {
    let out = daifuku()
        .args(args)
        .args([
            "--",
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            line,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn the_group_s_priority_reaches_the_program_and_what_it_starts() {
    let own = "[Console]::Out.Write((Get-Process -Id $PID).PriorityClass)";
    // A group the suite itself runs in caps every group in it at its own
    // priority, as a heavy job does at idle.
    let ceiling = powershell_in(&["group", "--priority", "normal"], own);
    let capped = |asked: &'static str| {
        let order = ["Idle", "BelowNormal", "Normal"];
        let rank = |p: &str| order.iter().position(|o| *o == p).unwrap();
        order[rank(asked).min(rank(&ceiling))]
    };
    assert_eq!(
        powershell_in(&["group", "--priority", "idle"], own),
        capped("Idle")
    );
    assert_eq!(
        powershell_in(&["group", "--priority", "below-normal"], own),
        capped("BelowNormal")
    );
    assert_eq!(powershell_in(&["heavy"], own), "Idle", "heavy runs at idle");
    // A child the program starts, asking to run higher, stays at idle.
    let child = "$p = Start-Process powershell.exe -ArgumentList '-NoProfile','-Command','Start-Sleep 5' -PassThru -WindowStyle Hidden; try { $p.PriorityClass = 'High' } catch {}; Start-Sleep -Milliseconds 300; $p.Refresh(); [Console]::Out.Write($p.PriorityClass); $p.Kill()";
    assert_eq!(
        powershell_in(&["group", "--priority", "idle"], child),
        "Idle"
    );
}

#[test]
fn the_exit_code_comes_back() {
    let status = daifuku()
        .args(["group", "--", "cmd.exe", "/d", "/c", "exit 7"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(7));
    let missing = daifuku()
        .args(["group", "--", "no-such-program-here"])
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("could not start"));
}

#[test]
fn what_the_program_leaves_running_ends_with_it() {
    let line = "$p = Start-Process ping.exe -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; [Console]::Out.Write($p.Id)";
    let pid = powershell_in(&["group"], line);
    let alive = || {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains("ping")
    };
    let start = Instant::now();
    while alive() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive(), "ping {pid} outlived its group");
}

#[test]
fn heavy_jobs_take_turns() {
    // Two jobs of about two seconds each, started together.
    let job = || {
        daifuku()
            .args(["heavy", "--", "ping.exe", "-n", "3", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let start = Instant::now();
    let (a, b) = (job(), job());
    let (a, b) = (a.wait_with_output().unwrap(), b.wait_with_output().unwrap());
    let took = start.elapsed();
    assert!(a.status.success() && b.status.success());
    assert!(
        took >= Duration::from_millis(3500),
        "they ran side by side: {took:?}"
    );
    let said =
        String::from_utf8_lossy(&a.stderr).into_owned() + &String::from_utf8_lossy(&b.stderr);
    assert!(
        said.contains("waiting for the heavy job before this one"),
        "{said}"
    );
}

#[test]
fn a_heavy_job_inside_a_heavy_job_has_its_turn_already() {
    let mut nested = daifuku()
        .args(["heavy", "--"])
        .arg(cli())
        .args(["heavy", "--", "cmd.exe", "/d", "/c", "exit 3"])
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = nested.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = nested.kill();
            panic!("the inner heavy job waited for the outer one's turn");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(3));
}
