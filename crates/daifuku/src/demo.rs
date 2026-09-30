//! `daifuku demo-agent`: a scripted stand-in for a coding agent.
//!
//! `daifuku demo` opens six of these, so anyone can see Daifuku work without
//! a real agent or an account. Each one reports through the same hook pipe a
//! real agent's hooks use, from inside its own terminal, so everything the
//! demo shows is the real path: the window lookup, the daemon, the borders.
//! It says plainly on screen that it is a demo.
//!
//! The script, per task: work for a few seconds, then either ask for
//! permission and wait for Enter, or fail once and retry, or simply finish;
//! rest, and take the next task. Which one is decided by the task and the
//! agent's number, so six agents never move in step.

use std::io::{BufRead, Write};
use std::time::Duration;

use daifuku_core::protocol::{HookMessage, to_line};
use daifuku_core::state::HookEvent;
use daifuku_win::pipe::{Pipe, send};
use daifuku_win::{console, process};

const TASKS: [(&str, &[&str]); 6] = [
    (
        "Add a dark mode toggle",
        &[
            "Reading src/theme.rs",
            "Reading src/settings.rs",
            "Planning 3 edits",
            "Editing src/theme.rs",
            "Running tests",
            "  48 passed",
        ],
    ),
    (
        "Fix the flaky login test",
        &[
            "Reading tests/login.rs",
            "Found a race on the session cookie",
            "Editing tests/login.rs",
            "Running tests 20 times",
            "  20 of 20 passed",
        ],
    ),
    (
        "Document the config file",
        &[
            "Reading src/config.rs",
            "Listing 14 keys",
            "Writing docs/config.md",
            "Checking every example parses",
        ],
    ),
    (
        "Speed up the image loader",
        &[
            "Profiling src/images.rs",
            "Decoding runs on the UI thread",
            "Moving it to a worker",
            "Benchmark: 180 ms to 40 ms",
        ],
    ),
    (
        "Refactor the grid planner",
        &[
            "Reading src/grid.rs",
            "Splitting shape and placement",
            "Editing src/grid.rs",
            "Running tests",
            "  52 passed",
        ],
    ),
    (
        "Update the dependencies",
        &[
            "Checking 31 crates",
            "4 have new versions",
            "Updating Cargo.lock",
            "Running tests",
            "  all green",
        ],
    ),
];

/// What a task ends in.
enum Ending {
    AsksPermission,
    FailsOnce,
    Finishes,
}

fn ending(agent: usize, task: usize) -> Ending {
    match (agent + task * 2) % 3 {
        0 => Ending::AsksPermission,
        1 => Ending::Finishes,
        _ => Ending::FailsOnce,
    }
}

struct Reporter {
    window: std::cell::Cell<Option<u64>>,
    session: String,
}

impl Reporter {
    fn report(&self, event: &str) {
        // Looked up again until found: the agent can start before its
        // terminal window exists.
        if self.window.get().is_none() {
            self.window.set(console::own_terminal_window());
        }
        let Some(window) = self.window.get() else {
            return;
        };
        let message = HookMessage {
            window,
            event: HookEvent {
                session_id: self.session.clone(),
                hook_event_name: event.to_owned(),
                ..HookEvent::default()
            },
        };
        if let Ok(line) = to_line(&message) {
            let _ = send(Pipe::Hook, &line, Duration::from_millis(200));
        }
    }
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn say(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

pub fn run(number: Option<usize>) {
    let me = process::current();
    // The fleet passes each terminal its number, so six agents never share
    // a script; started by hand, the process id stands in.
    let agent = number.map_or_else(
        || usize::try_from(me % 6).unwrap_or(0),
        |n| n.saturating_sub(1),
    );
    let reporter = Reporter {
        window: std::cell::Cell::new(console::own_terminal_window()),
        session: format!("demo-{me}"),
    };
    say("\x1b[2mdaifuku demo agent: scripted, not a real one. Ctrl+C to stop.\x1b[0m");
    say("");
    reporter.report("SessionStart");
    pause(600 + 250 * agent as u64);

    let stdin = std::io::stdin();
    for round in 0.. {
        let task = (agent + round) % TASKS.len();
        let (title, steps) = TASKS[task];
        say(&format!("\x1b[1m> {title}\x1b[0m"));
        reporter.report("UserPromptSubmit");
        for step in steps {
            pause(700 + 90 * ((agent + step.len()) % 7) as u64);
            say(&format!("  {step}"));
            reporter.report("PreToolUse");
        }
        match ending(agent, round) {
            Ending::AsksPermission => {
                say("");
                say("\x1b[33m  Allow the edit? Press Enter to approve.\x1b[0m");
                reporter.report("PermissionRequest");
                let mut line = String::new();
                if stdin.lock().read_line(&mut line).is_err() {
                    return;
                }
                reporter.report("PostToolUse");
                say("  Approved, applying");
                pause(900);
                reporter.report("Stop");
                say("\x1b[32m  Done.\x1b[0m");
            }
            Ending::FailsOnce => {
                reporter.report("StopFailure");
                say("\x1b[31m  API error: rate limited. Retrying in a few seconds.\x1b[0m");
                pause(5000);
                reporter.report("UserPromptSubmit");
                say("  Retrying");
                pause(1500);
                reporter.report("Stop");
                say("\x1b[32m  Done.\x1b[0m");
            }
            Ending::Finishes => {
                pause(500);
                reporter.report("Stop");
                say("\x1b[32m  Done.\x1b[0m");
            }
        }
        say("");
        pause(4000 + 700 * agent as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_agents_start_with_a_mix_of_endings() {
        let first: Vec<_> = (0..6).map(|a| ending(a, 0)).collect();
        assert!(first.iter().any(|e| matches!(e, Ending::AsksPermission)));
        assert!(first.iter().any(|e| matches!(e, Ending::FailsOnce)));
        assert!(first.iter().any(|e| matches!(e, Ending::Finishes)));
    }

    #[test]
    fn no_task_names_a_real_person_or_account() {
        for (title, steps) in TASKS {
            for text in std::iter::once(title).chain(steps.iter().copied()) {
                assert!(!text.contains('@') && !text.contains(r"C:\Users"), "{text}");
            }
        }
    }
}
