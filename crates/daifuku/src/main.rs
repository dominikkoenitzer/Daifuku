//! `daifuku`: the command line, and the hook agents call.
//!
//! ```text
//! daifuku open [fleet]   open a fleet, or bring it back
//! daifuku snap           put every fleet terminal back in its cell
//! daifuku close [fleet]  close a fleet's terminals
//! daifuku next           focus the agent that has waited longest
//! daifuku demo           open six scripted demo agents
//! daifuku status         what the daemon knows
//! daifuku reload         re-read the config
//! daifuku stop           stop the daemon
//! daifuku hook           read a hook event on stdin and report it (for agents)
//! daifuku schema         print the config file's JSON schema
//! daifuku config         print where the config file is
//! daifuku install        install for this user (administrator terminal)
//! daifuku uninstall      remove it again
//! daifuku doctor         check the setup
//! ```

#[cfg(windows)]
mod demo;
#[cfg(windows)]
mod install;

use std::io::Read;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use daifuku_core::config::Config;
use daifuku_core::protocol::Request;

#[derive(Parser)]
#[command(
    name = "daifuku",
    version,
    about = "Fleets of agent terminals, in a grid, coloured by what each agent is doing."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open a fleet, or bring it back if it is open. The first fleet in the
    /// config when no name is given.
    Open {
        /// The fleet's name.
        fleet: Option<String>,
    },
    /// Put every fleet terminal back in its cell.
    Snap,
    /// Close a fleet's terminals. The first fleet when no name is given.
    Close {
        /// The fleet's name.
        fleet: Option<String>,
    },
    /// Focus the agent that has waited longest for you.
    Next,
    /// Open six scripted demo agents: Daifuku without a real agent.
    Demo,
    /// One scripted demo agent. `daifuku demo` starts these.
    #[command(hide = true)]
    DemoAgent {
        /// Which of the fleet's terminals this is, from 1.
        number: Option<usize>,
    },
    /// Show fleets, agents, hotkeys and the config in use.
    Status {
        /// Print the raw JSON reply.
        #[arg(long)]
        json: bool,
    },
    /// Re-read the config file.
    Reload,
    /// Stop the daemon.
    Stop,
    /// Report one hook event, read from standard input. Agents call this;
    /// it never prints and always exits 0, so it can never disturb one.
    #[command(hide = true)]
    Hook,
    /// Print the config file's JSON schema.
    Schema,
    /// Print where the config file is.
    Config,
    /// Install for this user: Program Files, the logon task, Claude Code's
    /// hooks. Needs an administrator terminal.
    Install {
        /// Leave Claude Code's settings alone.
        #[arg(long)]
        no_hooks: bool,
        /// Do not start the daemon now; it starts at the next logon.
        #[arg(long)]
        no_start: bool,
    },
    /// Remove everything `install` added. Needs an administrator terminal.
    Uninstall {
        /// Also delete the config and the logs.
        #[arg(long)]
        purge: bool,
    },
    /// Check the setup and say how to fix anything wrong.
    Doctor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Hook => {
            hook();
            ExitCode::SUCCESS
        }
        Command::Schema => {
            print!("{}", Config::schema());
            ExitCode::SUCCESS
        }
        Command::Config => config_path(),
        Command::Install { no_hooks, no_start } => setup_result(install_cmd(no_hooks, no_start)),
        Command::Uninstall { purge } => setup_result(uninstall_cmd(purge)),
        Command::Doctor => {
            if doctor_cmd() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::Open { fleet } => control(&Request::Open { fleet }, false),
        Command::Snap => control(&Request::Snap, false),
        Command::Close { fleet } => control(&Request::Close { fleet }, false),
        Command::Next => control(&Request::Next, false),
        Command::Demo => control(&Request::Demo, false),
        Command::DemoAgent { number } => {
            demo_agent(number);
            ExitCode::SUCCESS
        }
        Command::Status { json } => control(&Request::Status, json),
        Command::Reload => control(&Request::Reload, false),
        Command::Stop => control(&Request::Stop, false),
    }
}

#[cfg(windows)]
fn demo_agent(number: Option<usize>) {
    demo::run(number);
}

#[cfg(not(windows))]
fn demo_agent(_: Option<usize>) {}

fn setup_result(r: anyhow::Result<()>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("daifuku: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn install_cmd(no_hooks: bool, no_start: bool) -> anyhow::Result<()> {
    install::install(&install::Options { no_hooks, no_start })
}

#[cfg(windows)]
fn uninstall_cmd(purge: bool) -> anyhow::Result<()> {
    install::uninstall(purge)
}

#[cfg(windows)]
fn doctor_cmd() -> bool {
    install::doctor()
}

#[cfg(not(windows))]
fn install_cmd(_: bool, _: bool) -> anyhow::Result<()> {
    anyhow::bail!("daifuku runs on Windows only")
}

#[cfg(not(windows))]
fn uninstall_cmd(_: bool) -> anyhow::Result<()> {
    anyhow::bail!("daifuku runs on Windows only")
}

#[cfg(not(windows))]
fn doctor_cmd() -> bool {
    false
}

/// The most a hook reads from standard input. Claude Code's events are a
/// few hundred bytes; the cap keeps a runaway producer from costing memory.
const MAX_HOOK_INPUT: u64 = 1024 * 1024;

#[cfg(windows)]
fn hook() {
    use daifuku_core::protocol::{HookMessage, to_line};
    use daifuku_core::state::{HookEvent, Transition};
    use daifuku_win::pipe::{Pipe, send};
    use daifuku_win::{console, process};

    let mut input = String::new();
    if std::io::stdin()
        .take(MAX_HOOK_INPUT)
        .read_to_string(&mut input)
        .is_err()
    {
        return;
    }
    let Ok(event) = HookEvent::from_hook_input(&input) else {
        return;
    };
    // An event that changes nothing is not worth a process walk.
    if event.transition() == Transition::Ignore {
        return;
    }
    let Some(window) = console::terminal_window(process::current()) else {
        return;
    };
    let Ok(line) = to_line(&HookMessage { window, event }) else {
        return;
    };
    // A short wait: an agent that waits for its hook must never notice a
    // daemon that is busy or not running.
    let _ = send(Pipe::Hook, &line, std::time::Duration::from_millis(100));
}

#[cfg(not(windows))]
fn hook() {
    let mut sink = String::new();
    let _ = std::io::stdin()
        .take(MAX_HOOK_INPUT)
        .read_to_string(&mut sink);
}

#[cfg(windows)]
fn config_path() -> ExitCode {
    match daifuku_win::paths::config_file() {
        Some(p) => {
            println!("{}", p.display());
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("daifuku: no ProgramData folder");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn config_path() -> ExitCode {
    eprintln!("daifuku runs on Windows only");
    ExitCode::FAILURE
}

#[cfg(windows)]
fn control(request: &Request, raw: bool) -> ExitCode {
    use daifuku_core::protocol::{Response, from_line, to_line};
    use daifuku_win::pipe::{Pipe, send};

    let Ok(line) = to_line(request) else {
        return ExitCode::FAILURE;
    };
    let reply = match send(Pipe::Control, &line, std::time::Duration::from_secs(2)) {
        Ok(Some(reply)) => reply,
        Ok(None) => return ExitCode::FAILURE,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("daifuku: {e}. Run it from an administrator terminal.");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("daifuku: {e}");
            return ExitCode::FAILURE;
        }
    };
    if raw {
        print!("{reply}");
        return ExitCode::SUCCESS;
    }
    match from_line::<Response>(&reply) {
        Ok(Response::Ok { message }) => {
            if let Some(m) = message {
                println!("{m}");
            }
            ExitCode::SUCCESS
        }
        Ok(Response::Status(status)) => {
            print_status(&status);
            ExitCode::SUCCESS
        }
        Ok(Response::Error { message }) => {
            eprintln!("daifuku: {message}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("daifuku: unreadable reply: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn control(_: &Request, _: bool) -> ExitCode {
    eprintln!("daifuku runs on Windows only");
    ExitCode::FAILURE
}

#[cfg_attr(not(windows), allow(dead_code))]
fn print_status(s: &daifuku_core::protocol::Status) {
    println!(
        "daifuku {}{}",
        s.version,
        if s.elevated {
            ", elevated"
        } else {
            ", not elevated"
        }
    );
    println!("config  {}", s.config);
    if !s.hotkeys.is_empty() {
        println!("\nhotkeys");
        for h in &s.hotkeys {
            println!("  {h}");
        }
    }
    println!("\nfleets");
    if s.fleets.is_empty() {
        println!("  none open");
    }
    for f in &s.fleets {
        println!(
            "  {} on {}: {}",
            f.name,
            f.monitor,
            terminals(f.windows.len())
        );
    }
    println!("\nagents");
    if s.agents.is_empty() {
        println!("  none reporting");
    }
    for a in &s.agents {
        println!(
            "  {:<8} {:>8}  {:#010x}  {}",
            a.state.name(),
            since(a.for_seconds),
            a.window,
            a.title
        );
    }
}

/// `1 terminal`, `3 terminals`: how many terminals a fleet has.
#[cfg_attr(not(windows), allow(dead_code))]
fn terminals(n: usize) -> String {
    if n == 1 {
        "1 terminal".to_owned()
    } else {
        format!("{n} terminals")
    }
}

/// `41s`, `3m 12s`, `2h 5m`: how long an agent has been in its state.
#[cfg_attr(not(windows), allow(dead_code))]
fn since(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn durations_read_like_a_person_would_say_them() {
        assert_eq!(since(0), "0s");
        assert_eq!(since(59), "59s");
        assert_eq!(since(192), "3m 12s");
        assert_eq!(since(7500), "2h 5m");
    }

    #[test]
    fn one_terminal_is_not_plural() {
        assert_eq!(terminals(1), "1 terminal");
        assert_eq!(terminals(0), "0 terminals");
        assert_eq!(terminals(6), "6 terminals");
    }

    #[test]
    fn the_hook_is_not_offered_to_people() {
        let cli = Cli::command();
        let hook = cli.find_subcommand("hook").unwrap();
        assert!(hook.is_hide_set());
    }

    #[test]
    fn open_takes_an_optional_fleet() {
        let cli = Cli::parse_from(["daifuku", "open", "mochi"]);
        assert!(matches!(cli.command, Command::Open { fleet: Some(ref f) } if f == "mochi"));
        let cli = Cli::parse_from(["daifuku", "open"]);
        assert!(matches!(cli.command, Command::Open { fleet: None }));
    }
}
