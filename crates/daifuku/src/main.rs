//! `daifuku`: the command line, and the hook agents call.
//!
//! ```text
//! daifuku open [fleet]   open a fleet, or bring it back
//! daifuku snap           put every fleet terminal back in its cell
//! daifuku close [fleet]  close a fleet's terminals
//! daifuku close --all    close every fleet's terminals
//! daifuku next           focus the agent that has waited longest
//! daifuku demo           open six scripted demo agents
//! daifuku spawn          open one agent terminal on its own, in a folder
//! daifuku restore        open the peers from before Windows restarted again
//! daifuku label          give an agent's terminal a label
//! daifuku wait           wait until an agent is done, or waiting for you
//! daifuku group -- <cmd> run a program in a group at one priority
//! daifuku heavy -- <cmd> run a build one at a time with the others, at idle
//! daifuku status         what the daemon knows
//! daifuku reload         re-read the config
//! daifuku stop           stop the daemon
//! daifuku hook           read a hook event on stdin and report it (for agents)
//! daifuku schema         print the config file's JSON schema
//! daifuku config         open the config file in your editor
//! daifuku config validate check a config file without the daemon
//! daifuku install        install for this user (administrator terminal)
//! daifuku uninstall      remove it again
//! daifuku install --dry-run, uninstall --dry-run   what they would do
//! daifuku doctor         check the setup
//! ```
//!
//! Double-clicked in Explorer, with no command, it offers to install. The
//! Uninstall button in Settings > Apps runs `uninstall --from-settings`.
//!
//! Every command but the hidden ones takes `--json`, install and uninstall
//! only with `--dry-run`: one JSON document on standard output, errors
//! included, described in `docs/json-output.md`.

#[cfg(windows)]
mod demo;
#[cfg(windows)]
mod install;
mod local;
mod output;
mod validate;
mod wait;

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use daifuku_core::config::Config;
use daifuku_core::protocol::Request;
use output::{ErrorCode, ErrorDoc, Finding};

// The `--json` flag, the same on every command that takes it. A doc comment
// here would become the about text of each command it is flattened into.
#[derive(Args, Debug, Clone, Copy, Default)]
struct Format {
    /// Print one JSON document on standard output, errors included.
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
#[command(
    name = "daifuku",
    version,
    about = "Fleets of agent terminals, in a grid, coloured by what each agent is doing.",
    after_help = "The daemon runs with your own rights unless the config sets \"elevated\": true. Then the commands that talk to it, open to stop, need an administrator terminal, and from any other, status says only what it can tell without the daemon."
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
        #[command(flatten)]
        format: Format,
    },
    /// Put every fleet terminal back in its cell.
    Snap {
        #[command(flatten)]
        format: Format,
    },
    /// Close a fleet's terminals. The first fleet when no name is given.
    Close {
        /// The fleet's name.
        fleet: Option<String>,
        /// Close every open fleet instead.
        #[arg(long, conflicts_with = "fleet")]
        all: bool,
        /// Close only this terminal: a fleet's, a peer or one an agent
        /// reports from, by its window handle or its label.
        #[arg(long, value_name = "WINDOW_OR_LABEL", conflicts_with_all = ["fleet", "all"])]
        terminal: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Focus the agent that has waited longest for you.
    Next {
        #[command(flatten)]
        format: Format,
    },
    /// Open six scripted demo agents: Daifuku without a real agent.
    Demo {
        #[command(flatten)]
        format: Format,
    },
    /// Open one agent terminal on its own, in the folder it works in, with
    /// the command, profile and administrator setting of a fleet. The
    /// variables an agent sets for itself are left out of its environment.
    Spawn {
        /// The folder it works in; this one when left out.
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        /// Its title, for people and for `daifuku status`.
        #[arg(long)]
        label: Option<String>,
        /// A session to resume, by its id.
        #[arg(long)]
        resume: Option<String>,
        /// The fleet whose command it runs; the first one when left out.
        #[arg(long)]
        fleet: Option<String>,
        /// The monitor to put it on, as a fleet's `monitor` takes it.
        #[arg(long)]
        monitor: Option<String>,
        /// The part of the monitor it fills: full, left, right, top or
        /// bottom. With neither this nor --monitor it takes a free cell of
        /// its fleet while the fleet is open, else the fleet's monitor and
        /// area.
        #[arg(long)]
        area: Option<String>,
        /// Run it in a group at this priority, idle, below-normal or normal,
        /// instead of its fleet's.
        #[arg(long, value_parser = parse_priority)]
        priority: Option<daifuku_core::config::Priority>,
        /// Take a cell of the fleet's grid, the first free one or the one
        /// numbered, and be one of its terminals: placed, snapped and closed
        /// with it.
        #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "free", conflicts_with_all = ["monitor", "area"])]
        cell: Option<daifuku_core::peer::Cell>,
        #[command(flatten)]
        format: Format,
    },
    /// Open the peers `daifuku spawn` opened that were still open when
    /// Windows last stopped, each resuming its last session in its folder.
    Restore {
        /// Only list them.
        #[arg(long)]
        dry_run: bool,
        /// Forget them instead of opening them.
        #[arg(long)]
        forget: bool,
        #[command(flatten)]
        format: Format,
    },
    /// Give an agent's terminal a label, which `status` shows and `wait`
    /// takes. Without a label, take it away.
    Label {
        /// The terminal's window handle, as `status` lists it, in decimal or
        /// as 0x and hex.
        #[arg(value_parser = parse_window)]
        window: u64,
        /// The label.
        label: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Wait until an agent's terminal reaches a state: by default until its
    /// agent's next turn ends, done, waiting for you or failed, or the agent
    /// exits.
    Wait {
        /// The terminal: its window handle or its label.
        target: String,
        /// The state to wait for.
        #[arg(long, value_enum, default_value = "turn")]
        until: wait::Until,
        /// Give up after this many seconds.
        #[arg(long, default_value_t = 3600)]
        timeout: u64,
        #[command(flatten)]
        format: Format,
    },
    /// Run a program in a group of its own: everything it starts runs at the
    /// group's priority, which none of it can raise, and ends when the
    /// program does or its terminal closes.
    Group {
        /// idle, below-normal or normal.
        #[arg(long, default_value = "normal", value_parser = parse_priority)]
        priority: daifuku_core::config::Priority,
        /// The program and its arguments, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<std::ffi::OsString>,
    },
    /// Run a build or a test run in a group, one at a time with every other
    /// heavy job in this session, at idle priority unless told otherwise.
    Heavy {
        /// idle, below-normal or normal.
        #[arg(long, default_value = "idle", value_parser = parse_priority)]
        priority: daifuku_core::config::Priority,
        /// The program and its arguments, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<std::ffi::OsString>,
    },
    /// The terminal window process `pid` runs in, as a number on one line,
    /// or nothing. The daemon asks this rather than attach to an agent's
    /// console itself, which would end it if that console closed.
    #[command(hide = true)]
    ConsoleWindow {
        /// The process.
        pid: u32,
    },
    /// One scripted demo agent. `daifuku demo` starts these.
    #[command(hide = true)]
    DemoAgent {
        /// Which of the fleet's terminals this is, from 1.
        number: Option<usize>,
    },
    /// Show fleets, agents, hotkeys and the config in use. From a terminal
    /// that is not elevated, when the daemon runs elevated: whether Daifuku
    /// is installed and running, and the config.
    Status {
        #[command(flatten)]
        format: Format,
    },
    /// Re-read the config file.
    Reload {
        #[command(flatten)]
        format: Format,
    },
    /// Stop the daemon.
    Stop {
        #[command(flatten)]
        format: Format,
    },
    /// Report one hook event, read from standard input. Agents call this;
    /// it never prints and always exits 0, so it can never disturb one.
    #[command(hide = true)]
    Hook,
    /// Print the config file's JSON schema.
    Schema {
        #[command(flatten)]
        format: Format,
    },
    /// Open the config file in your editor, asking Windows for
    /// administrator rights when this terminal may not write it. Every key in
    /// it is optional.
    #[command(args_conflicts_with_subcommands = true)]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
        /// Print where the config file is instead.
        #[arg(long)]
        path: bool,
        #[command(flatten)]
        format: Format,
    },
    /// Install for this user: Program Files, the hooks for Claude Code and
    /// Codex, and a logon task only for an elevated daemon. Needs an
    /// administrator terminal.
    Install {
        /// Leave Claude Code's and Codex's settings alone.
        #[arg(long)]
        no_hooks: bool,
        /// Stop a running daemon and do not start it again; the next
        /// `daifuku` command or agent hook starts it.
        #[arg(long)]
        no_start: bool,
        /// Wait for Enter at the end, so the window the offer to install
        /// opened stays until it is read.
        #[arg(long, hide = true)]
        pause: bool,
        /// Print what install would do and change nothing. Needs no
        /// administrator terminal.
        #[arg(long)]
        dry_run: bool,
        /// With --dry-run, print the plan as one JSON document.
        #[arg(long, requires = "dry_run")]
        json: bool,
    },
    /// Remove the program, any logon task and Daifuku's hooks. Keeps the
    /// config, the logs and the saved state unless --purge. Needs an
    /// administrator terminal.
    Uninstall {
        /// Also delete the config, the logs and the saved state.
        #[arg(long)]
        purge: bool,
        /// Print what uninstall would do and change nothing. Needs no
        /// administrator terminal.
        #[arg(long)]
        dry_run: bool,
        /// With --dry-run, print the plan as one JSON document.
        #[arg(long, requires = "dry_run")]
        json: bool,
        /// Started by the Uninstall button in Settings > Apps: ask Windows
        /// for administrator rights when there are none, and keep the
        /// window open a while at the end.
        #[arg(long, hide = true, conflicts_with = "dry_run")]
        from_settings: bool,
    },
    /// Check the setup and say how to fix anything wrong.
    Doctor {
        #[command(flatten)]
        format: Format,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Check a config file the way the daemon reads it, without the daemon
    /// and without administrator rights. Changes nothing.
    Validate {
        /// The file to check; the config file in use when left out.
        file: Option<std::path::PathBuf>,
        #[command(flatten)]
        format: Format,
    },
}

fn main() -> ExitCode {
    #[cfg(windows)]
    if std::env::args_os().len() <= 1
        && let Some(code) = from_explorer()
    {
        return code;
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return usage_error(&e, std::env::args_os().any(|a| a == "--json")),
    };
    match cli.command {
        Command::Hook => {
            hook();
            ExitCode::SUCCESS
        }
        // The schema is JSON already; --json leaves it as it is.
        Command::Schema { .. } => {
            print!("{}", Config::schema());
            ExitCode::SUCCESS
        }
        Command::Config {
            action: Some(ConfigAction::Validate { file, format }),
            ..
        } => config_validate(file, format.json),
        Command::Config {
            path: true,
            format,
            ..
        } => config_path(format.json),
        Command::Config {
            path: false,
            format: Format { json: false },
            ..
        } => setup_result(edit_config().map(|file| {
            println!("opened       {}", file.display());
            println!(
                "\nEvery key is optional; what you leave out keeps its default. Daifuku applies the file within two seconds of each save."
            );
        })),
        Command::Config {
            path: false,
            format: Format { json: true },
            ..
        } => match edit_config() {
            Ok(_) => {
                output::print_json(&output::Done { ok: true });
                ExitCode::SUCCESS
            }
            Err(e) => {
                let code = if cfg!(windows) {
                    ErrorCode::Config
                } else {
                    ErrorCode::Unsupported
                };
                fail_json(code, &format!("{e:#}"))
            }
        },
        Command::Install {
            dry_run: true,
            no_hooks,
            no_start,
            json,
            ..
        } => dry_run_cmd(install_dry_run(no_hooks, no_start), json),
        Command::Uninstall {
            dry_run: true,
            purge,
            json,
            ..
        } => dry_run_cmd(uninstall_dry_run(purge), json),
        Command::Install {
            no_hooks,
            no_start,
            pause,
            ..
        } => {
            let result = install_cmd(no_hooks, no_start);
            if pause && result.is_ok() {
                println!("{INSTALLED}");
            }
            let code = setup_result(result);
            if pause {
                wait_for_enter();
            }
            code
        }
        Command::Uninstall {
            purge,
            from_settings: true,
            ..
        } => uninstall_from_settings(purge),
        Command::Uninstall { purge, .. } => setup_result(uninstall_cmd(purge)),
        Command::Doctor { format } => doctor_cmd(format.json),
        Command::Open { fleet, format } => control(&Request::Open { fleet }, format.json),
        Command::Snap { format } => control(&Request::Snap, format.json),
        Command::Close {
            terminal: Some(terminal),
            format,
            ..
        } => control(&Request::CloseTerminal { terminal }, format.json),
        Command::Close {
            all: true, format, ..
        } => control(&Request::CloseAll, format.json),
        Command::Close { fleet, format, .. } => control(&Request::Close { fleet }, format.json),
        Command::Next { format } => control(&Request::Next, format.json),
        Command::Demo { format } => control(&Request::Demo, format.json),
        Command::Spawn {
            dir,
            label,
            resume,
            fleet,
            monitor,
            area,
            priority,
            cell,
            format,
        } => match spawn_request(dir, label, resume, fleet, monitor.as_deref(), area.as_deref()) {
            Ok(spawn) => control(
                &Request::Spawn(daifuku_core::peer::Spawn {
                    priority,
                    cell,
                    ..spawn
                }),
                format.json,
            ),
            Err(message) if format.json => fail_json(ErrorCode::Usage, &message),
            Err(message) => {
                eprintln!("daifuku: {message}");
                ExitCode::from(2)
            }
        },
        Command::DemoAgent { number } => {
            demo_agent(number);
            ExitCode::SUCCESS
        }
        Command::ConsoleWindow { pid } => console_window(pid),
        Command::Status { format } => status_cmd(format.json),
        Command::Restore {
            dry_run,
            forget,
            format,
        } => control(&Request::Restore { dry_run, forget }, format.json),
        Command::Label {
            window,
            label,
            format,
        } => control(&Request::Label { window, label }, format.json),
        Command::Wait {
            target,
            until,
            timeout,
            format,
        } => wait_cmd(&target, until, timeout, format.json),
        Command::Group { priority, command } => group_cmd(priority, false, &command),
        Command::Heavy { priority, command } => group_cmd(priority, true, &command),
        Command::Reload { format } => control(&Request::Reload, format.json),
        Command::Stop { format } => control(&Request::Stop, format.json),
    }
}

/// A command line that did not parse. With `--json` anywhere in it, the
/// error is the JSON error with code `usage`; help and version requests,
/// and everything without `--json`, go the way clap always takes them.
fn usage_error(e: &clap::Error, json: bool) -> ExitCode {
    use clap::error::ErrorKind;
    let asked = matches!(
        e.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
    if !json || asked {
        e.exit();
    }
    output::print_json(&ErrorDoc::new(ErrorCode::Usage, &usage_message(e)));
    // What clap exits with on a usage error.
    ExitCode::from(2)
}

/// The first line of clap's error, without its `error: ` in front. A line
/// that ends in a colon takes the next one along, which names what it means.
fn usage_message(e: &clap::Error) -> String {
    let text = e.render().to_string();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or_default();
    let first = first.strip_prefix("error: ").unwrap_or(first);
    match lines.next() {
        Some(next) if first.ends_with(':') => format!("{first} {next}"),
        _ => first.to_owned(),
    }
}

/// Prints the JSON error and returns the failure exit code.
fn fail_json(code: ErrorCode, message: &str) -> ExitCode {
    output::print_json(&ErrorDoc::new(code, message));
    ExitCode::FAILURE
}

/// What `daifuku` started with no command does.
#[derive(Debug, PartialEq, Eq)]
enum Bare {
    /// Print the help, as there is a terminal to type a command in.
    Help,
    /// Offer to install: Explorer started it, in a console of its own.
    /// `elevate` when it has to ask Windows for administrator rights first.
    Offer { elevate: bool },
}

/// What to do with no command: offer to install only when this process is
/// alone in its console and someone can answer at the keyboard. In a
/// terminal, or with input from a file or a pipe, it prints the help.
#[cfg_attr(not(windows), allow(dead_code))]
fn bare(own_console: bool, typed_input: bool, elevated: bool) -> Bare {
    if own_console && typed_input {
        Bare::Offer { elevate: !elevated }
    } else {
        Bare::Help
    }
}

/// The last word of an install started by a double click.
#[cfg_attr(not(windows), allow(dead_code))]
const INSTALLED: &str = "\nDaifuku is installed. In a new terminal, `daifuku doctor` checks the setup and `daifuku config` opens the settings.";

/// The offer to install, when Explorer started this with no command.
/// `None` when this is not that case and the help should print instead.
#[cfg(windows)]
fn from_explorer() -> Option<ExitCode> {
    use daifuku_win::{elevate, process};
    use std::io::IsTerminal;

    let Bare::Offer { elevate: ask } = bare(
        elevate::console_is_own(),
        std::io::stdin().is_terminal(),
        process::current_is_elevated(),
    ) else {
        return None;
    };
    let Ok(exe) = std::env::current_exe() else {
        return Some(ExitCode::FAILURE);
    };
    // The daifuku.exe a release publishes on its own carries the daemon;
    // the one in the zip needs it next to it.
    if install::daemon_for(&exe).is_none() {
        println!("daifukud.exe is not in this folder, and Daifuku needs both programs.");
        println!(
            "If you opened daifuku.exe inside the zip, extract the zip first (right-click it, Extract All), then open daifuku.exe in the new folder."
        );
        wait_for_enter();
        return Some(ExitCode::FAILURE);
    }
    println!(
        "This installs Daifuku: it copies it to Program Files, starts it and adds its hooks to Claude Code and Codex."
    );
    println!(
        "Windows asks for permission next. Press Enter to install, or close this window to stop."
    );
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
        return Some(ExitCode::FAILURE);
    }
    if !ask {
        let result = install_cmd(false, false);
        if result.is_ok() {
            println!("{INSTALLED}");
        }
        let code = setup_result(result);
        wait_for_enter();
        return Some(code);
    }
    // The install runs in a window of its own, which waits for Enter at the
    // end; this one has nothing left to say and closes.
    match elevate::run_elevated(&exe, "install --pause") {
        Ok(()) => Some(ExitCode::SUCCESS),
        Err(e) => {
            if elevate::declined(&e) {
                println!("Nothing was installed: Windows was not given permission.");
            } else {
                println!("Windows could not start the install: {e}");
            }
            wait_for_enter();
            Some(ExitCode::FAILURE)
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn wait_for_enter() {
    println!("\nPress Enter to close this window.");
    let _ = std::io::stdin().read_line(&mut String::new());
}

/// How long the window the Uninstall button in Settings opened stays after
/// the removal, unless Enter closes it sooner.
#[cfg_attr(not(windows), allow(dead_code))]
const LINGER: std::time::Duration = std::time::Duration::from_secs(30);

/// Waits for Enter, or [`LINGER`] at most: nobody may be there to press
/// it, and a window that waited for good would keep Settings waiting too.
/// Without a keyboard to read from it does not wait at all.
#[cfg_attr(not(windows), allow(dead_code))]
fn wait_a_while() {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return;
    }
    println!(
        "\nPress Enter to close this window; it closes by itself in {} seconds.",
        LINGER.as_secs()
    );
    let (pressed, enter) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read_line(&mut String::new());
        let _ = pressed.send(());
    });
    let _ = enter.recv_timeout(LINGER);
}

/// `uninstall` as the Uninstall button in Settings > Apps starts it: in a
/// console window of its own, maybe without administrator rights. Without
/// them it asks Windows, runs itself again elevated in a window of its own
/// and waits for that one, so Settings lists the apps again only once
/// Daifuku is gone. Elevated, it removes Daifuku, says what it removed and
/// leaves the window up a while to be read.
#[cfg(windows)]
fn uninstall_from_settings(purge: bool) -> ExitCode {
    use daifuku_win::{elevate, process};

    // Settings may start it in any folder, the one about to be deleted
    // included, and a folder a process is in cannot be deleted.
    let _ = std::env::set_current_dir(std::env::temp_dir());
    if process::current_is_elevated() {
        println!("Removing Daifuku.\n");
        let result = uninstall_cmd(purge);
        if result.is_ok() {
            println!("\nDaifuku is removed.");
        }
        let code = setup_result(result);
        wait_a_while();
        return code;
    }
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::FAILURE;
    };
    println!("Removing Daifuku. Windows asks for permission next.");
    let mut args = install::FROM_SETTINGS.to_owned();
    if purge {
        args.push_str(" --purge");
    }
    match elevate::start_elevated(&exe, &args) {
        Ok(started) => {
            // The removal shows in the new window; this one has nothing
            // more to say.
            if elevate::console_is_own() {
                elevate::leave_console();
            }
            match started.wait() {
                Ok(0) => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            }
        }
        Err(e) => {
            if elevate::declined(&e) {
                println!("Nothing was removed: Windows was not given permission.");
            } else {
                println!("Windows could not start the removal: {e}");
            }
            wait_a_while();
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn uninstall_from_settings(purge: bool) -> ExitCode {
    setup_result(uninstall_cmd(purge))
}

/// Opens the config in the program Windows opens `.json` files with, or in
/// Notepad, and returns the file. An elevated daemon's config only
/// administrators may change, so from a terminal that is not elevated the
/// editor starts after the Windows prompt; an ordinary daemon's the user may
/// write, and the editor starts with the user's own rights.
#[cfg(windows)]
fn edit_config() -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context;
    use daifuku_win::{elevate, paths, process};

    let file = paths::config_file().context("no ProgramData folder")?;
    if !file.is_file() {
        anyhow::bail!(
            "there is no config at {} yet; `daifuku install` writes one",
            file.display()
        );
    }
    let prompt = !process::current_is_elevated() && !writable(&file);
    let notepad = paths::system_dir().map(|d| d.join("notepad.exe"));
    let quoted = format!("\"{}\"", file.display());
    let mut last = None;
    for editor in elevate::opens(".json").into_iter().chain(notepad) {
        let started = if prompt {
            elevate::run_elevated(&editor, &quoted)
        } else {
            elevate::run(&editor, &quoted)
        };
        match started {
            Ok(()) => return Ok(file),
            Err(e) if elevate::declined(&e) => {
                anyhow::bail!("Windows was not given permission, so the config stays as it is");
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.map_or_else(
        || anyhow::anyhow!("found no editor"),
        |e| anyhow::anyhow!("could not open an editor: {e}"),
    ))
}

/// Whether this process may write `file`. Opened for writing and closed
/// again, which changes nothing in it.
#[cfg(windows)]
fn writable(file: &std::path::Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(file).is_ok()
}

#[cfg(not(windows))]
fn edit_config() -> anyhow::Result<std::path::PathBuf> {
    anyhow::bail!("daifuku runs on Windows only")
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
fn install_dry_run(no_hooks: bool, no_start: bool) -> anyhow::Result<output::Plan> {
    install::install_dry_run(&install::Options { no_hooks, no_start })
}

#[cfg(windows)]
fn uninstall_dry_run(purge: bool) -> anyhow::Result<output::Plan> {
    install::uninstall_dry_run(purge)
}

#[cfg(not(windows))]
fn install_dry_run(_: bool, _: bool) -> anyhow::Result<output::Plan> {
    anyhow::bail!("daifuku runs on Windows only")
}

#[cfg(not(windows))]
fn uninstall_dry_run(_: bool) -> anyhow::Result<output::Plan> {
    anyhow::bail!("daifuku runs on Windows only")
}

/// Prints the plan of a dry run, as lines or as one JSON document.
fn dry_run_cmd(plan: anyhow::Result<output::Plan>, json: bool) -> ExitCode {
    match plan {
        Ok(plan) if json => {
            output::print_json(&plan);
            ExitCode::SUCCESS
        }
        Ok(plan) => {
            for line in plan.lines() {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) if json => {
            let code = if cfg!(windows) {
                ErrorCode::Internal
            } else {
                ErrorCode::Unsupported
            };
            fail_json(code, &format!("{e:#}"))
        }
        Err(e) => setup_result(Err(e)),
    }
}

/// Runs the doctor. Its exit code says whether every check passed, with
/// `--json` too.
#[cfg(windows)]
fn doctor_cmd(json: bool) -> ExitCode {
    let ok = if json {
        let mut report = output::DoctorReport::new();
        install::doctor(&mut |f| report.add(f));
        output::print_json(&report);
        report.ok
    } else {
        install::doctor(&mut |f| println!("{}", f.line()))
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
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
fn doctor_cmd(json: bool) -> ExitCode {
    if json {
        fail_json(ErrorCode::Unsupported, "daifuku runs on Windows only")
    } else {
        ExitCode::FAILURE
    }
}

/// Whether the config on disk asks for an elevated daemon. An elevated one
/// may be started only without a prompt at logon by its task, never raised
/// here, so the CLI leaves it to the task.
#[cfg(windows)]
fn config_wants_elevated() -> bool {
    use daifuku_core::config::{self, Config};
    daifuku_win::paths::config_file()
        .and_then(|p| std::fs::read(&p).ok())
        .and_then(|b| config::decode(&b).ok())
        .and_then(|t| Config::from_json(&t).ok())
        .is_some_and(|c| c.elevated)
}

/// Starts the daemon with the user's own rights when none is listening, for
/// the default install that has no logon task. `daifukud.exe` sits next to
/// this program. Returns whether the daemon answers afterwards.
///
/// It runs with the user's own rights also when this command runs as
/// administrator, and holds nothing of this process: not its output, which
/// an agent reads to its end, nor its folder or its environment.
///
/// An elevated daemon is never started here: it would need a prompt, and the
/// config that asks for it keeps a logon task for exactly that reason. Nor
/// one while install or uninstall is changing the binaries.
#[cfg(windows)]
fn start_daemon() -> bool {
    use daifuku_win::pipe::{Pipe, send};
    if config_wants_elevated() {
        return false;
    }
    let Some(daemon) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("daifukud.exe")))
        .filter(|d| d.is_file())
    else {
        return false;
    };
    if daifuku_win::setup::install_running()
        || daifuku_win::process::start_detached(&daemon).is_err()
    {
        return false;
    }
    // Wait, but not for long, for it to begin listening.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if send(Pipe::Hook, "{}\n", std::time::Duration::from_millis(100)).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

#[cfg(windows)]
fn hook() {
    use daifuku_core::protocol::{HookMessage, to_line};
    use daifuku_core::state::{HookEvent, Transition};
    use daifuku_win::pipe::{Pipe, send};
    use daifuku_win::{console, process};

    // Stamped before anything slow, reading the event included, so the
    // order of the stamps is the order the agent ran its hooks in. Not by
    // the wall clock: set back, it would make every later event look older
    // than the last one applied.
    let at = Some(daifuku_win::clock::ticks());
    // Read as it comes, with no cap: a tool event for a large edit carries
    // the whole file, and only the few fields Daifuku needs are kept.
    let Ok(mut event) = HookEvent::from_hook_input(std::io::stdin().lock()) else {
        return;
    };
    event.daifuku_at = at;
    // An event that changes nothing is not worth a process walk.
    if event.transition() == Transition::Ignore {
        return;
    }
    // A long notification must not make the line too long for the pipe.
    event.trim();
    let Some((window, agent)) = console::terminal_window_of(process::current()) else {
        return;
    };
    let message = HookMessage {
        window,
        event,
        pid: agent,
    };
    let Ok(line) = to_line(&message) else {
        return;
    };
    // A short wait: an agent that waits for its hook must never notice a
    // daemon that is busy or not running.
    let sent = send(Pipe::Hook, &line, std::time::Duration::from_millis(100));
    // The default install has no logon task, so the first event of a session
    // finds no daemon: start one and let the agent carry on. Later events
    // find it running and pay nothing for this.
    if matches!(&sent, Err(e) if e.kind() == std::io::ErrorKind::NotFound) && start_daemon() {
        let _ = send(Pipe::Hook, &line, std::time::Duration::from_secs(1));
    }
}

#[cfg(not(windows))]
fn hook() {
    let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
}

#[cfg(windows)]
fn config_path(json: bool) -> ExitCode {
    match daifuku_win::paths::config_file() {
        Some(p) if json => {
            output::print_json(&output::ConfigPath {
                path: p.display().to_string(),
            });
            ExitCode::SUCCESS
        }
        Some(p) => {
            println!("{}", p.display());
            ExitCode::SUCCESS
        }
        None if json => fail_json(ErrorCode::Config, "no ProgramData folder"),
        None => {
            eprintln!("daifuku: no ProgramData folder");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn config_path(json: bool) -> ExitCode {
    if json {
        return fail_json(ErrorCode::Unsupported, "daifuku runs on Windows only");
    }
    eprintln!("daifuku runs on Windows only");
    ExitCode::FAILURE
}

/// Checks a config file, the one in use when `file` is `None`, and says
/// whether the daemon would load it. Exits 1 when it would not.
fn config_validate(file: Option<std::path::PathBuf>, json: bool) -> ExitCode {
    let fail = |code: ErrorCode, message: &str| {
        if json {
            fail_json(code, message)
        } else {
            eprintln!("daifuku: {message}");
            ExitCode::FAILURE
        }
    };
    let file = match file.map_or_else(default_config_file, Ok) {
        Ok(file) => file,
        Err((code, message)) => return fail(code, &message),
    };
    let path = file.display().to_string();
    match validate::file(&file) {
        Ok(()) if json => {
            output::print_json(&output::Validated { ok: true, path });
            ExitCode::SUCCESS
        }
        Ok(()) => {
            println!(
                "{}",
                Finding::Check {
                    name: path,
                    fix: None
                }
                .line()
            );
            ExitCode::SUCCESS
        }
        Err(p) if json || p.code != ErrorCode::InvalidConfig => fail(p.code, &p.message),
        Err(p) => {
            println!(
                "{}",
                Finding::Check {
                    name: path,
                    fix: Some(p.message),
                }
                .line()
            );
            ExitCode::FAILURE
        }
    }
}

/// The config file the daemon reads.
#[cfg(windows)]
fn default_config_file() -> Result<std::path::PathBuf, (ErrorCode, String)> {
    daifuku_win::paths::config_file()
        .ok_or_else(|| (ErrorCode::Config, "no ProgramData folder".to_owned()))
}

#[cfg(not(windows))]
fn default_config_file() -> Result<std::path::PathBuf, (ErrorCode, String)> {
    Err((
        ErrorCode::Unsupported,
        "daifuku runs on Windows only; name the file to check".to_owned(),
    ))
}

#[cfg(windows)]
fn control(request: &Request, json: bool) -> ExitCode {
    use daifuku_core::protocol::to_line;
    use daifuku_win::pipe::{Pipe, send};

    let Ok(line) = to_line(request) else {
        return if json {
            fail_json(ErrorCode::Internal, "could not write the request")
        } else {
            ExitCode::FAILURE
        };
    };
    let mut result = send(Pipe::Control, &line, std::time::Duration::from_secs(2));
    // No logon task on the default install: the first command of a session
    // starts the daemon, then asks again. Not one that only asks whether it
    // runs or tells it to stop.
    let starts = !matches!(request, Request::Status | Request::Stop);
    if starts
        && matches!(&result, Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        && start_daemon()
    {
        result = send(Pipe::Control, &line, std::time::Duration::from_secs(2));
    }
    match result {
        Ok(Some(reply)) => answer(&reply, request, json),
        Ok(None) if json => fail_json(ErrorCode::Ipc, "the daemon did not answer"),
        Ok(None) => ExitCode::FAILURE,
        Err(e) => {
            let elevated = daifuku_win::process::current_is_elevated();
            let message = control_error(&e, elevated, &command_line(request, json));
            control_failed(json, control_code(&e, elevated), &message)
        }
    }
}

/// A failure of a command for the daemon: as JSON with --json, else on
/// stderr, as it always has been.
#[cfg(windows)]
fn control_failed(json: bool, code: ErrorCode, message: &str) -> ExitCode {
    if json {
        fail_json(code, message)
    } else {
        eprintln!("daifuku: {message}");
        ExitCode::FAILURE
    }
}

/// Prints the daemon's `reply` to `request`.
#[cfg(windows)]
fn answer(reply: &str, request: &Request, json: bool) -> ExitCode {
    use daifuku_core::protocol::{Response, from_line, to_line};

    let fail = |code: ErrorCode, message: &str| control_failed(json, code, message);
    match from_line::<Response>(reply) {
        Ok(Response::Ok { .. }) if json => {
            output::print_json(&output::Done { ok: true });
            ExitCode::SUCCESS
        }
        Ok(Response::Ok { message }) => {
            if let Some(m) = message {
                println!("{m}");
            }
            ExitCode::SUCCESS
        }
        Ok(Response::Status(status)) if json => {
            // Written again from what was read, so every documented field is
            // there even when an older daemon left one out. A script reads
            // this through a pipe, which PowerShell decodes in the console's
            // code page, not UTF-8; escaped, a title arrives intact.
            print!(
                "{}",
                output::ascii_json(&to_line(&Response::Status(status)).unwrap_or_default())
            );
            ExitCode::SUCCESS
        }
        Ok(Response::Status(status)) => {
            print_status(&status);
            ExitCode::SUCCESS
        }
        Ok(spawned @ Response::Spawned { .. }) if json => {
            print!(
                "{}",
                output::ascii_json(&to_line(&spawned).unwrap_or_default())
            );
            ExitCode::SUCCESS
        }
        Ok(Response::Spawned {
            window,
            label,
            fleet,
            cell,
            ..
        }) => {
            println!(
                "{}",
                spawned_line(window, label.as_deref(), fleet.as_deref(), cell)
            );
            ExitCode::SUCCESS
        }
        Ok(restored @ Response::Restored { .. }) if json => {
            print!(
                "{}",
                output::ascii_json(&to_line(&restored).unwrap_or_default())
            );
            ExitCode::SUCCESS
        }
        Ok(Response::Restored { peers }) => {
            print_restored(&peers, request);
            ExitCode::SUCCESS
        }
        Ok(Response::Error { message }) => fail(ErrorCode::Daemon, &message),
        Err(e) => fail(ErrorCode::Ipc, &format!("unreadable reply: {e}")),
    }
}

/// `daifuku status`. A daemon with the user's own rights answers any of the
/// user's terminals; an elevated one only an administrator terminal. So from
/// a terminal that is not elevated, when the daemon does not answer it, this
/// reports what it can tell without the pipe and exits 0.
#[cfg(windows)]
fn status_cmd(json: bool) -> ExitCode {
    use daifuku_core::protocol::to_line;
    use daifuku_win::pipe::{Pipe, send};

    let request = Request::Status;
    if daifuku_win::process::current_is_elevated() {
        return control(&request, json);
    }
    if let Ok(line) = to_line(&request)
        && let Ok(Some(reply)) = send(Pipe::Control, &line, std::time::Duration::from_secs(2))
    {
        return answer(&reply, &request, json);
    }
    let status = local::gather(&install::Here, &command_line(&request, json));
    let (text, code) = local::report(&status, json);
    println!("{text}");
    ExitCode::from(code)
}

#[cfg(not(windows))]
fn status_cmd(json: bool) -> ExitCode {
    control(&Request::Status, json)
}

/// What to say when the daemon could not be asked. An elevated daemon's
/// control pipe admits only elevated processes, as a request can open
/// administrator terminals; a daemon with the user's own rights admits the
/// user's terminals. So when Windows itself refused the open from a terminal
/// that is not elevated, the daemon runs elevated, and the message ends with
/// `command`, the command to run again in an administrator terminal.
#[cfg_attr(not(windows), allow(dead_code))]
fn control_error(e: &std::io::Error, elevated: bool, command: &str) -> String {
    if refused_by_windows(e, elevated) {
        format!(
            "the daemon runs elevated and answers only an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: {command}"
        )
    } else {
        e.to_string()
    }
}

/// Whether Windows refused a terminal that is not elevated the control pipe,
/// as it does an elevated daemon's. The client's own refusal of a server it
/// does not trust is no Windows error code, and is said as it is.
#[cfg_attr(not(windows), allow(dead_code))]
fn refused_by_windows(e: &std::io::Error, elevated: bool) -> bool {
    !elevated && e.kind() == std::io::ErrorKind::PermissionDenied && e.raw_os_error().is_some()
}

/// The command line that sends `request`, as a person would type it, with
/// `--json` when it was given.
#[cfg_attr(not(windows), allow(dead_code))]
fn command_line(request: &Request, json: bool) -> String {
    let with_fleet = |command: &str, fleet: &Option<String>| match fleet {
        Some(name) => format!("{command} {}", shell_word(name)),
        None => command.to_owned(),
    };
    let command = match request {
        Request::Open { fleet } => with_fleet("open", fleet),
        Request::Close { fleet } => with_fleet("close", fleet),
        Request::CloseAll => "close --all".to_owned(),
        Request::CloseTerminal { terminal } => {
            format!("close --terminal {}", shell_word(terminal))
        }
        Request::Snap => "snap".to_owned(),
        Request::Next => "next".to_owned(),
        Request::Demo => "demo".to_owned(),
        Request::Spawn(_) => "spawn".to_owned(),
        Request::Label { window, label } => match label {
            Some(l) => format!("label {window} {}", shell_word(l)),
            None => format!("label {window}"),
        },
        Request::Restore { dry_run, forget } => match (dry_run, forget) {
            (true, true) => "restore --forget --dry-run".to_owned(),
            (true, false) => "restore --dry-run".to_owned(),
            (false, true) => "restore --forget".to_owned(),
            (false, false) => "restore".to_owned(),
        },
        Request::Status => "status".to_owned(),
        Request::Reload => "reload".to_owned(),
        Request::Stop => "stop".to_owned(),
    };
    if json {
        format!("daifuku {command} --json")
    } else {
        format!("daifuku {command}")
    }
}

/// `word` as one argument for PowerShell or the command prompt: as it is
/// when it is plain, in double quotes otherwise.
#[cfg_attr(not(windows), allow(dead_code))]
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if plain {
        word.to_owned()
    } else {
        format!("\"{word}\"")
    }
}

/// The request `daifuku spawn` sends, from its arguments: the folder made
/// absolute, as the daemon runs in another one, and the monitor and area
/// checked as the config checks them.
fn spawn_request(
    dir: Option<std::path::PathBuf>,
    label: Option<String>,
    resume: Option<String>,
    fleet: Option<String>,
    monitor: Option<&str>,
    area: Option<&str>,
) -> Result<daifuku_core::peer::Spawn, String> {
    use daifuku_core::config::{Area, MonitorPick};
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir().map_err(|e| format!("no current folder: {e}"))?,
    };
    let directory = std::path::absolute(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let monitor = match monitor {
        Some(text) => match text.parse::<MonitorPick>() {
            Ok(MonitorPick::Device(d)) if !d.starts_with(r"\\.\") => {
                return Err(format!(
                    r"monitor `{d}` is not portrait, landscape, primary, secondary, cursor or a device name such as \\.\DISPLAY2"
                ));
            }
            Ok(pick) => Some(pick),
            Err(never) => match never {},
        },
        None => None,
    };
    let area = area
        .map(str::parse::<Area>)
        .transpose()
        .map_err(|e| e.to_string())?;
    let spawn = daifuku_core::peer::Spawn {
        directory,
        label,
        resume,
        fleet,
        monitor,
        area,
        priority: None,
        cell: None,
    };
    match spawn.problem() {
        Some(problem) => Err(problem),
        None => Ok(spawn),
    }
}

/// What `daifuku spawn` says about the terminal it opened: its label, and
/// the fleet and cell it took when it asked for one.
fn spawned_line(
    window: u64,
    label: Option<&str>,
    fleet: Option<&str>,
    cell: Option<u32>,
) -> String {
    let what = label.unwrap_or("a terminal");
    match (fleet, cell) {
        (Some(f), Some(n)) => format!("opened {what} in cell {n} of {f} ({window:#010x})"),
        _ => format!("opened {what} ({window:#010x})"),
    }
}

/// The error code for a daemon that could not be asked, matching
/// [`control_error`].
#[cfg_attr(not(windows), allow(dead_code))]
fn control_code(e: &std::io::Error, elevated: bool) -> ErrorCode {
    if e.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::DaemonNotRunning
    } else if refused_by_windows(e, elevated) {
        ErrorCode::NotElevated
    } else {
        ErrorCode::Ipc
    }
}

/// `daifuku console-window`: prints the window `pid`'s terminal is, or
/// nothing and exits 1.
#[cfg(windows)]
fn console_window(pid: u32) -> ExitCode {
    match daifuku_win::console::terminal_window(pid) {
        Some(w) => {
            println!("{w}");
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

#[cfg(not(windows))]
fn console_window(_: u32) -> ExitCode {
    ExitCode::FAILURE
}

/// A priority as `group` and `heavy` take it.
fn parse_priority(text: &str) -> Result<daifuku_core::config::Priority, String> {
    text.parse()
}

/// `daifuku group` and `daifuku heavy`: puts this process in a new job at
/// `priority`, waits its turn first with `queue`, runs `command` there and
/// exits with its exit code. Exiting closes the job, which ends whatever the
/// command left running.
#[cfg(windows)]
fn group_cmd(
    priority: daifuku_core::config::Priority,
    queue: bool,
    command: &[std::ffi::OsString],
) -> ExitCode {
    use daifuku_win::job;

    let Some((program, args)) = command.split_first() else {
        eprintln!("daifuku: no program to run");
        return ExitCode::from(2);
    };
    let group = job::Job::new(priority).and_then(|j| j.add_this_process().map(|()| j));
    let Ok(_group) = group.inspect_err(|e| eprintln!("daifuku: could not make the group: {e}"))
    else {
        return ExitCode::FAILURE;
    };
    job::outlive_ctrl_c();
    // Inside a heavy job already, which holds the turn for its whole tree.
    let nested = std::env::var_os(job::IN_HEAVY).is_some();
    let _turn = if queue && !nested {
        match job::wait_turn(|| eprintln!("daifuku: waiting for the heavy job before this one")) {
            Ok(turn) => Some(turn),
            Err(e) => {
                eprintln!("daifuku: could not take a turn at the heavy jobs: {e}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    let mut command = std::process::Command::new(program);
    command.args(args);
    if queue {
        command.env(job::IN_HEAVY, "1");
    }
    let code = match command.spawn() {
        Ok(mut child) => {
            job::running(child.id());
            child.wait().ok().and_then(|s| s.code()).unwrap_or(1)
        }
        Err(e) => {
            eprintln!(
                "daifuku: could not start {}: {e}",
                program.to_string_lossy()
            );
            std::process::exit(1);
        }
    };
    // Once in the group, every way out is an exit, never a return: the
    // group closing as it is dropped would end this process before it set
    // its exit code, as 0.
    std::process::exit(code)
}

#[cfg(not(windows))]
fn group_cmd(_: daifuku_core::config::Priority, _: bool, _: &[std::ffi::OsString]) -> ExitCode {
    eprintln!("daifuku runs on Windows only");
    ExitCode::FAILURE
}

/// A window handle as `daifuku label` takes it.
fn parse_window(text: &str) -> Result<u64, String> {
    wait::window_number(text).ok_or_else(|| format!("`{text}` is not a window handle"))
}

/// Sends `request` to the daemon and reads its answer, or says why it could
/// not, as [`control`] does.
#[cfg(windows)]
fn ask(
    request: &Request,
    json: bool,
) -> Result<daifuku_core::protocol::Response, (ErrorCode, String)> {
    use daifuku_core::protocol::{Response, from_line, to_line};
    use daifuku_win::pipe::{Pipe, send};

    let line = to_line(request).map_err(|_| {
        (
            ErrorCode::Internal,
            "could not write the request".to_owned(),
        )
    })?;
    let reply = match send(Pipe::Control, &line, std::time::Duration::from_secs(2)) {
        Ok(Some(reply)) => reply,
        Ok(None) => return Err((ErrorCode::Ipc, "the daemon did not answer".to_owned())),
        Err(e) => {
            let elevated = daifuku_win::process::current_is_elevated();
            return Err((
                control_code(&e, elevated),
                control_error(&e, elevated, &command_line(request, json)),
            ));
        }
    };
    from_line::<Response>(&reply).map_err(|e| (ErrorCode::Ipc, format!("unreadable reply: {e}")))
}

/// `daifuku wait`: asks for the status once a second until `target` reaches
/// `until`, its terminal closes, or `timeout` seconds pass.
#[cfg(windows)]
fn wait_cmd(target: &str, until: wait::Until, timeout: u64, json: bool) -> ExitCode {
    use daifuku_core::protocol::Response;
    use wait::Step;

    let fail = |code: ErrorCode, message: &str| {
        if json {
            fail_json(code, message)
        } else {
            eprintln!("daifuku: {message}");
            ExitCode::FAILURE
        }
    };
    let start = std::time::Instant::now();
    let mut seen = wait::Seen::default();
    loop {
        let status = match ask(&Request::Status, json) {
            Ok(Response::Status(s)) => s,
            Ok(Response::Error { message }) => return fail(ErrorCode::Daemon, &message),
            Ok(_) => return fail(ErrorCode::Ipc, "the daemon did not answer with its status"),
            Err((code, message)) => return fail(code, &message),
        };
        let done = |window, state: &'static str, label, session_id| {
            let waited = wait::Waited {
                result: "waited",
                window,
                state,
                label,
                session_id,
                seconds: start.elapsed().as_secs(),
            };
            if json {
                println!("{}", output::ascii_json(&output::to_json(&waited)));
            } else {
                let name = waited
                    .label
                    .clone()
                    .unwrap_or_else(|| format!("{window:#010x}"));
                println!("{name} is {state}");
            }
            ExitCode::SUCCESS
        };
        match wait::step(&status, target, until, &mut seen) {
            Step::Reached(window, state, label, session_id) => {
                return done(window, state.name(), label, session_id);
            }
            Step::Closed(window) => return done(window, "closed", None, None),
            Step::Exited(window) => return done(window, "exited", None, None),
            Step::Waiting => {}
            Step::Unknown => {
                return fail(
                    ErrorCode::Daemon,
                    &format!("no agent terminal is called `{target}`"),
                );
            }
        }
        if start.elapsed().as_secs() >= timeout {
            return fail(
                ErrorCode::Timeout,
                &format!("`{target}` did not get there in {timeout} seconds"),
            );
        }
        // Twice a second, so a short turn is seen working.
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(not(windows))]
fn wait_cmd(_: &str, _: wait::Until, _: u64, json: bool) -> ExitCode {
    control(&Request::Status, json)
}

#[cfg(not(windows))]
fn control(_: &Request, json: bool) -> ExitCode {
    if json {
        return fail_json(ErrorCode::Unsupported, "daifuku runs on Windows only");
    }
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
    // Most urgent first and the title before the handle, so a screen reader
    // reaches the agent that needs you in the first words it reads.
    for a in by_urgency(&s.agents) {
        println!(
            "  {:<8} {:>8}  {}  ({:#010x})",
            a.state.name(),
            since(a.for_seconds),
            a.title,
            a.window
        );
        for line in details(a) {
            println!("           {line}");
        }
    }
    if !s.peers.is_empty() {
        println!("\npeers");
    }
    for p in &s.peers {
        println!(
            "  {}  {}  ({:#010x})",
            p.label.as_deref().unwrap_or("-"),
            p.directory,
            p.window
        );
    }
}

/// What `daifuku restore` did, one line a peer.
fn print_restored(peers: &[daifuku_core::protocol::RestoredPeer], request: &Request) {
    let (dry_run, forget) = match request {
        Request::Restore { dry_run, forget } => (*dry_run, *forget),
        _ => (false, false),
    };
    if peers.is_empty() {
        println!("no peers to open again");
    }
    for p in peers {
        let name = p.label.as_deref().unwrap_or(&p.directory);
        let what = match (&p.window, &p.error) {
            (Some(w), _) => format!("opened {name} ({w:#010x})"),
            (None, Some(e)) => format!("did not open {name}: {e}"),
            (None, None) if dry_run && forget => format!("would forget {name}"),
            (None, None) if forget => format!("forgot {name}"),
            (None, None) => format!("would open {name}"),
        };
        println!("{what}, session {} in {}", p.session_id, p.directory);
    }
}

/// The agents most urgent first: waiting, failed, working, done, and within
/// each the one in that state longest first.
#[cfg_attr(not(windows), allow(dead_code))]
fn by_urgency(
    agents: &[daifuku_core::protocol::AgentWindow],
) -> Vec<&daifuku_core::protocol::AgentWindow> {
    let mut sorted: Vec<_> = agents.iter().collect();
    sorted.sort_by_key(|a| (std::cmp::Reverse(a.state), std::cmp::Reverse(a.for_seconds)));
    sorted
}

/// The lines under an agent in `daifuku status`: the folder it works in,
/// and what it asks while it waits. Both are what a hook said, so their line
/// breaks, other control characters and the bidi controls, which reorder how
/// a line reads, become spaces and each stays on its line.
#[cfg_attr(not(windows), allow(dead_code))]
fn details(a: &daifuku_core::protocol::AgentWindow) -> Vec<String> {
    let one_line = |text: &str| -> String {
        let text: String = text
            .chars()
            .map(|c| {
                let bidi = matches!(
                    c,
                    '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                );
                if c.is_control() || bidi { ' ' } else { c }
            })
            .collect();
        text.trim().to_owned()
    };
    let mut lines = Vec::new();
    if let Some(cwd) = &a.cwd {
        lines.push(format!("in {}", one_line(cwd)));
    }
    if a.state == daifuku_core::state::AgentState::Waiting
        && let Some(m) = &a.last_message
    {
        lines.push(format!("asks: {}", one_line(m)));
    }
    lines
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

    #[test]
    fn status_lists_the_agent_that_needs_you_first() {
        use daifuku_core::protocol::AgentWindow;
        use daifuku_core::state::AgentState;
        let agent = |window, state, for_seconds| AgentWindow {
            window,
            title: String::new(),
            state,
            for_seconds,
            label: None,
            fleet: None,
            session_id: None,
            cwd: None,
            last_tool: None,
            last_message: None,
            transcript_path: None,
        };
        let agents = [
            agent(1, AgentState::Done, 90),
            agent(2, AgentState::Waiting, 5),
            agent(3, AgentState::Working, 10),
            agent(4, AgentState::Waiting, 40),
            agent(5, AgentState::Failed, 1),
        ];
        let order: Vec<u64> = by_urgency(&agents).iter().map(|a| a.window).collect();
        assert_eq!(order, [4, 2, 5, 3, 1]);
    }

    #[test]
    fn status_says_where_an_agent_works_and_what_it_asks() {
        use daifuku_core::protocol::AgentWindow;
        use daifuku_core::state::AgentState;
        let mut a = AgentWindow {
            window: 1,
            title: String::new(),
            state: AgentState::Waiting,
            for_seconds: 0,
            label: None,
            fleet: None,
            session_id: None,
            cwd: Some("site".into()),
            last_tool: Some("Bash".into()),
            last_message: Some("may I run\r\nthis? ".into()),
            transcript_path: None,
        };
        assert_eq!(details(&a), ["in site", "asks: may I run  this?"]);
        // Once it works again, the question is old news.
        a.state = AgentState::Working;
        assert_eq!(details(&a), ["in site"]);
        // The folder is what a hook said too, so it cannot break the listing.
        a.cwd = Some("C:\\src\nfake line\u{1b}[2J".into());
        assert_eq!(details(&a), ["in C:\\src fake line [2J"]);
        // Nor reorder what the rest of the line reads like.
        a.cwd = Some("C:\\src\\\u{202e}gpj.exe".into());
        assert_eq!(details(&a), ["in C:\\src\\ gpj.exe"]);
        for c in [
            '\u{61c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
            '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            a.cwd = Some(format!("a{c}b"));
            assert_eq!(details(&a), ["in a b"], "{:#x}", c as u32);
        }
        a.cwd = None;
        assert!(details(&a).is_empty());
    }
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
    fn a_refused_control_pipe_asks_for_an_administrator_terminal() {
        // What the pipe client returns when Windows refuses the open.
        let denied = std::io::Error::from_raw_os_error(5);
        assert_eq!(
            control_error(&denied, false, "daifuku status --json"),
            "the daemon runs elevated and answers only an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status --json"
        );
        // A daemon with the user's rights that is busy is said as it is: an
        // administrator terminal would not help.
        let busy = std::io::Error::new(std::io::ErrorKind::TimedOut, "Daifuku is busy");
        assert_eq!(
            control_error(&busy, false, "daifuku status"),
            "Daifuku is busy"
        );
        // So is a server the client itself turned away.
        let stranger = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the process answering is not the daemon",
        );
        assert_eq!(
            control_error(&stranger, false, "daifuku status"),
            "the process answering is not the daemon"
        );
        // A daemon that is not running is said as it is.
        let missing = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Daifuku is not running; `daifuku doctor` says why",
        );
        assert_eq!(
            control_error(&missing, false, "daifuku status"),
            "Daifuku is not running; `daifuku doctor` says why"
        );
        // Elevated already, the terminal is not the problem.
        let spoofed = std::io::Error::from_raw_os_error(5);
        assert_eq!(
            control_error(&spoofed, true, "daifuku status"),
            spoofed.to_string()
        );
    }

    #[test]
    fn the_command_to_run_again_is_the_one_that_was_asked() {
        assert_eq!(command_line(&Request::Status, false), "daifuku status");
        assert_eq!(
            command_line(&Request::Status, true),
            "daifuku status --json"
        );
        assert_eq!(
            command_line(
                &Request::Open {
                    fleet: Some("my agents".into())
                },
                true
            ),
            "daifuku open \"my agents\" --json"
        );
        assert_eq!(
            command_line(
                &Request::Open {
                    fleet: Some("mochi".into())
                },
                false
            ),
            "daifuku open mochi"
        );
        assert_eq!(
            command_line(&Request::Open { fleet: None }, false),
            "daifuku open"
        );
        assert_eq!(
            command_line(
                &Request::Close {
                    fleet: Some("agents".into())
                },
                false
            ),
            "daifuku close agents"
        );
        assert_eq!(
            command_line(&Request::CloseAll, false),
            "daifuku close --all"
        );
        assert_eq!(command_line(&Request::Stop, false), "daifuku stop");
        assert_eq!(command_line(&Request::Next, true), "daifuku next --json");
    }

    #[test]
    fn spawn_says_the_cell_a_peer_took() {
        assert_eq!(
            spawned_line(0x1234, Some("Site QA"), Some("crew"), Some(2)),
            "opened Site QA in cell 2 of crew (0x00001234)"
        );
        assert_eq!(
            spawned_line(0x1234, None, None, None),
            "opened a terminal (0x00001234)"
        );
    }

    #[test]
    fn a_fleet_name_is_quoted_only_when_it_needs_it() {
        assert_eq!(shell_word("agents-2.b_c"), "agents-2.b_c");
        assert_eq!(shell_word("my agents"), "\"my agents\"");
        assert_eq!(shell_word("a&b"), "\"a&b\"");
        assert_eq!(shell_word(""), "\"\"");
    }

    #[test]
    fn every_request_is_named_as_the_command_line_takes_it() {
        // The command printed must parse back to the request it came from.
        for request in [
            Request::Open { fleet: None },
            Request::Open {
                fleet: Some("mochi".into()),
            },
            Request::Snap,
            Request::Close { fleet: None },
            Request::Close {
                fleet: Some("agents".into()),
            },
            Request::CloseAll,
            Request::CloseTerminal {
                terminal: "0x10064e".into(),
            },
            Request::Next,
            Request::Demo,
            Request::Status,
            Request::Reload,
            Request::Stop,
        ] {
            let line = command_line(&request, true);
            let cli =
                Cli::try_parse_from(line.split(' ')).unwrap_or_else(|e| panic!("{line}: {e}"));
            assert_eq!(json_flag(&cli.command), Some(true), "{line}");
            let back = match cli.command {
                Command::Open { fleet, .. } => Request::Open { fleet },
                Command::Snap { .. } => Request::Snap,
                Command::Close {
                    terminal: Some(terminal),
                    ..
                } => Request::CloseTerminal { terminal },
                Command::Close { all: true, .. } => Request::CloseAll,
                Command::Close { fleet, .. } => Request::Close { fleet },
                Command::Next { .. } => Request::Next,
                Command::Demo { .. } => Request::Demo,
                Command::Status { .. } => Request::Status,
                Command::Reload { .. } => Request::Reload,
                Command::Stop { .. } => Request::Stop,
                _ => panic!("{line} is not a daemon command"),
            };
            assert_eq!(back, request, "{line}");
        }
    }

    #[test]
    fn json_for_a_script_is_ascii_and_reads_the_same() {
        let reply = "{\"title\":\"\u{2733} Gr\u{fc}ezi \u{1f600}\",\"a\\\\b\":1}\n";
        let ascii = output::ascii_json(reply);
        assert_eq!(
            ascii,
            "{\"title\":\"\\u2733 Gr\\u00fcezi \\ud83d\\ude00\",\"a\\\\b\":1}\n"
        );
        let read = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(read(&ascii), read(reply));
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
    fn only_a_console_of_its_own_with_a_keyboard_offers_to_install() {
        assert_eq!(bare(true, true, false), Bare::Offer { elevate: true });
        assert_eq!(bare(true, true, true), Bare::Offer { elevate: false });
        // A terminal: its shell shares the console.
        assert_eq!(bare(false, true, false), Bare::Help);
        assert_eq!(bare(false, true, true), Bare::Help);
        // Input from a file or a pipe: nobody to press Enter.
        assert_eq!(bare(true, false, false), Bare::Help);
        assert_eq!(bare(false, false, true), Bare::Help);
    }

    #[test]
    fn config_opens_the_file_and_prints_its_path_on_request() {
        let cli = Cli::parse_from(["daifuku", "config"]);
        assert!(matches!(cli.command, Command::Config { path: false, .. }));
        let cli = Cli::parse_from(["daifuku", "config", "--path"]);
        assert!(matches!(cli.command, Command::Config { path: true, .. }));
    }

    #[test]
    fn config_validate_takes_an_optional_file() {
        let cli = Cli::parse_from(["daifuku", "config", "validate"]);
        assert!(matches!(
            cli.command,
            Command::Config {
                action: Some(ConfigAction::Validate { file: None, .. }),
                path: false,
                ..
            }
        ));
        let cli = Cli::parse_from(["daifuku", "config", "validate", "x.json"]);
        let Command::Config {
            action:
                Some(ConfigAction::Validate {
                    file: Some(file), ..
                }),
            ..
        } = cli.command
        else {
            panic!("config validate with a file did not parse");
        };
        assert_eq!(file, std::path::Path::new("x.json"));
    }

    #[test]
    fn config_options_do_not_mix_with_validate() {
        for args in [
            &["config", "--path", "validate"][..],
            &["config", "--json", "validate"],
            &["config", "validate", "--path"],
            &["config", "validate", "a.json", "b.json"],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("daifuku").chain(args.iter().copied()))
                    .is_err(),
                "{args:?} parsed"
            );
        }
    }

    #[test]
    fn the_pause_after_install_is_not_offered_to_people() {
        let cli = Cli::command();
        let install = cli.find_subcommand("install").unwrap();
        let pause = install
            .get_arguments()
            .find(|a| a.get_id() == "pause")
            .unwrap();
        assert!(pause.is_hide_set());
        let cli = Cli::parse_from(["daifuku", "install", "--pause"]);
        assert!(matches!(cli.command, Command::Install { pause: true, .. }));
    }

    /// What the Uninstall button in Settings > Apps runs parses, and is
    /// not offered to people.
    #[cfg(windows)]
    #[test]
    fn the_uninstall_settings_runs_parses_and_is_hidden() {
        let cli = Cli::command();
        let uninstall = cli.find_subcommand("uninstall").unwrap();
        let from_settings = uninstall
            .get_arguments()
            .find(|a| a.get_id() == "from_settings")
            .unwrap();
        assert!(from_settings.is_hide_set());
        let args = install::FROM_SETTINGS.split(' ');
        let cli = Cli::try_parse_from(std::iter::once("daifuku").chain(args)).unwrap();
        assert!(matches!(
            cli.command,
            Command::Uninstall {
                from_settings: true,
                purge: false,
                dry_run: false,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["daifuku", "uninstall", "--from-settings", "--dry-run"]).is_err(),
            "a dry run is for a terminal"
        );
    }

    /// The `--json` flag of a command, `None` for one that has none.
    fn json_flag(command: &Command) -> Option<bool> {
        match command {
            Command::Open { format, .. }
            | Command::Snap { format }
            | Command::Close { format, .. }
            | Command::Next { format }
            | Command::Demo { format }
            | Command::Spawn { format, .. }
            | Command::Restore { format, .. }
            | Command::Label { format, .. }
            | Command::Wait { format, .. }
            | Command::Status { format }
            | Command::Reload { format }
            | Command::Stop { format }
            | Command::Schema { format }
            | Command::Config {
                action: Some(ConfigAction::Validate { format, .. }),
                ..
            }
            | Command::Config {
                action: None,
                format,
                ..
            }
            | Command::Doctor { format } => Some(format.json),
            Command::Install { json, .. } | Command::Uninstall { json, .. } => Some(*json),
            Command::DemoAgent { .. }
            | Command::Hook
            | Command::Group { .. }
            | Command::ConsoleWindow { .. }
            | Command::Heavy { .. } => None,
        }
    }

    #[test]
    fn spawn_sends_an_absolute_folder_and_checks_what_it_can() {
        use daifuku_core::config::{Area, MonitorPick};
        let here = std::env::current_dir().unwrap();
        let s = spawn_request(None, None, None, None, None, None).unwrap();
        assert_eq!(s.directory, here);
        assert!(!s.placed());
        let s = spawn_request(
            Some("sub".into()),
            Some("Site QA".into()),
            Some("abc-1".into()),
            Some("crew".into()),
            Some("Landscape"),
            Some("RIGHT"),
        )
        .unwrap();
        assert_eq!(s.directory, here.join("sub"));
        assert_eq!(s.monitor, Some(MonitorPick::Landscape));
        assert_eq!(s.area, Some(Area::Right));
        assert_eq!(
            (s.label.as_deref(), s.resume.as_deref(), s.fleet.as_deref()),
            (Some("Site QA"), Some("abc-1"), Some("crew"))
        );
        let s = spawn_request(None, None, None, None, Some(r"\\.\DISPLAY2"), None).unwrap();
        assert_eq!(s.monitor, Some(MonitorPick::Device(r"\\.\DISPLAY2".into())));
        for (monitor, area, resume, says) in [
            (Some("sideways"), None, None, "monitor `sideways`"),
            (None, Some("middle"), None, "`middle` is not an area"),
            (None, None, Some("a;b"), "`a;b` is not a session id"),
        ] {
            let e =
                spawn_request(None, None, resume.map(Into::into), None, monitor, area).unwrap_err();
            assert!(e.starts_with(says), "{e}");
        }
        use daifuku_core::peer::Cell;
        let cell = |args: &[&str]| match Cli::try_parse_from(
            ["daifuku", "spawn", "--fleet", "crew"].iter().chain(args),
        )
        .map(|c| c.command)
        {
            Ok(Command::Spawn { cell, .. }) => Ok(cell),
            Ok(_) => panic!("not spawn"),
            Err(e) => Err(e.kind()),
        };
        assert_eq!(cell(&[]), Ok(None));
        assert_eq!(cell(&["--cell"]), Ok(Some(Cell::Free)));
        assert_eq!(cell(&["--cell", "2"]), Ok(Some(Cell::At(2))));
        assert_eq!(cell(&["--cell", "--json"]), Ok(Some(Cell::Free)));
        assert!(cell(&["--cell", "0"]).is_err());
        // Its fleet's grid places it; a place of its own is a mistake.
        assert!(cell(&["--cell", "--area", "left"]).is_err());
        assert!(cell(&["--cell", "2", "--monitor", "portrait"]).is_err());
        let cli = Cli::try_parse_from(["daifuku", "spawn", "--area", "left", "--dir", r"C:\x"]);
        assert!(matches!(
            cli.unwrap().command,
            Command::Spawn { area: Some(a), dir: Some(_), .. } if a == "left"
        ));
    }

    #[test]
    fn every_command_that_reports_or_acts_takes_json_after_its_name() {
        for args in [
            &["status", "--json"][..],
            &["doctor", "--json"],
            &["schema", "--json"],
            &["config", "--json"],
            &["config", "--path", "--json"],
            &["config", "--json", "--path"],
            &["config", "validate", "--json"],
            &["config", "validate", "--json", "x.json"],
            &["config", "validate", "x.json", "--json"],
            &["open", "--json"],
            &["open", "agents", "--json"],
            &["open", "--json", "agents"],
            &["snap", "--json"],
            &["close", "--json"],
            &["close", "agents", "--json"],
            &["close", "--all", "--json"],
            &["next", "--json"],
            &["demo", "--json"],
            &["spawn", "--json"],
            &["restore", "--json"],
            &["label", "0x10", "site", "--json"],
            &["wait", "site", "--json"],
            &[
                "wait",
                "--until",
                "done",
                "--timeout",
                "60",
                "site",
                "--json",
            ],
            &["restore", "--dry-run", "--json"],
            &["spawn", "--dir", "x", "--label", "a b", "--json"],
            &["reload", "--json"],
            &["stop", "--json"],
            &["install", "--dry-run", "--json"],
            &["install", "--json", "--dry-run", "--no-hooks"],
            &["uninstall", "--dry-run", "--json"],
            &["uninstall", "--purge", "--json", "--dry-run"],
        ] {
            let cli = Cli::try_parse_from(std::iter::once("daifuku").chain(args.iter().copied()))
                .unwrap_or_else(|e| panic!("{args:?}: {e}"));
            assert_eq!(json_flag(&cli.command), Some(true), "{args:?}");
        }
        let cli = Cli::try_parse_from(["daifuku", "status"]).unwrap();
        assert_eq!(json_flag(&cli.command), Some(false));
        let cli = Cli::try_parse_from(["daifuku", "config", "--path", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Config {
                action: None,
                path: true,
                format: Format { json: true }
            }
        ));
    }

    #[test]
    fn json_is_refused_where_it_does_not_belong_with_a_usage_error() {
        for args in [&["--json", "status"][..], &["hook", "--json"]] {
            let e = Cli::try_parse_from(std::iter::once("daifuku").chain(args.iter().copied()))
                .err()
                .unwrap_or_else(|| panic!("{args:?} parsed"));
            assert_eq!(
                e.kind(),
                clap::error::ErrorKind::UnknownArgument,
                "{args:?}"
            );
            assert_eq!(usage_message(&e), "unexpected argument '--json' found");
        }
        // Install and uninstall take it only for a dry run.
        for command in ["install", "uninstall"] {
            let e = Cli::try_parse_from(["daifuku", command, "--json"])
                .err()
                .unwrap_or_else(|| panic!("{command} --json parsed"));
            assert_eq!(
                e.kind(),
                clap::error::ErrorKind::MissingRequiredArgument,
                "{command}"
            );
            assert!(usage_message(&e).contains("--dry-run"), "{command}");
        }
        let e = Cli::try_parse_from(["daifuku", "open", "a", "b", "--json"])
            .err()
            .unwrap();
        assert!(!usage_message(&e).starts_with("error: "));
        assert!(!usage_message(&e).is_empty());
    }

    #[test]
    fn help_with_json_is_still_help() {
        let e = Cli::try_parse_from(["daifuku", "status", "--json", "--help"])
            .err()
            .unwrap();
        assert_eq!(e.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn a_daemon_that_could_not_be_asked_has_a_code_for_each_reason() {
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "not running");
        assert_eq!(control_code(&missing, false), ErrorCode::DaemonNotRunning);
        assert_eq!(control_code(&missing, true), ErrorCode::DaemonNotRunning);
        let denied = std::io::Error::from_raw_os_error(5);
        assert_eq!(control_code(&denied, false), ErrorCode::NotElevated);
        assert_eq!(control_code(&denied, true), ErrorCode::Ipc);
        let busy = std::io::Error::new(std::io::ErrorKind::TimedOut, "busy");
        assert_eq!(control_code(&busy, false), ErrorCode::Ipc);
    }

    #[cfg(windows)]
    #[test]
    fn a_config_the_user_may_write_opens_without_a_prompt() {
        let file = std::env::temp_dir().join(format!("daifuku-config-{}.json", std::process::id()));
        std::fs::write(&file, "{}").unwrap();
        assert!(writable(&file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}");
        let mut read_only = std::fs::metadata(&file).unwrap().permissions();
        read_only.set_readonly(true);
        std::fs::set_permissions(&file, read_only.clone()).unwrap();
        assert!(!writable(&file));
        // On Windows this clears the read-only attribute and nothing more.
        #[allow(clippy::permissions_set_readonly_false)]
        read_only.set_readonly(false);
        std::fs::set_permissions(&file, read_only).unwrap();
        std::fs::remove_file(&file).unwrap();
        assert!(!writable(&file));
    }

    #[test]
    fn open_takes_an_optional_fleet() {
        let cli = Cli::parse_from(["daifuku", "open", "mochi"]);
        assert!(matches!(cli.command, Command::Open { fleet: Some(ref f), .. } if f == "mochi"));
        let cli = Cli::parse_from(["daifuku", "open"]);
        assert!(matches!(cli.command, Command::Open { fleet: None, .. }));
    }
}
