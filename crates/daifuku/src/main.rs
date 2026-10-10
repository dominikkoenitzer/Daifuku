//! `daifuku`: the command line, and the hook agents call.
//!
//! ```text
//! daifuku open [fleet]   open a fleet, or bring it back
//! daifuku snap           put every fleet terminal back in its cell
//! daifuku close [fleet]  close a fleet's terminals
//! daifuku close --all    close every fleet's terminals
//! daifuku next           focus the agent that has waited longest
//! daifuku demo           open six scripted demo agents
//! daifuku status         what the daemon knows
//! daifuku reload         re-read the config
//! daifuku stop           stop the daemon
//! daifuku hook           read a hook event on stdin and report it (for agents)
//! daifuku schema         print the config file's JSON schema
//! daifuku config         open the config file in your editor
//! daifuku config validate check a config file without the daemon
//! daifuku install        install for this user (administrator terminal)
//! daifuku uninstall      remove it again
//! daifuku doctor         check the setup
//! ```
//!
//! Double-clicked in Explorer, with no command, it offers to install.
//!
//! Every command but install, uninstall and the hidden ones takes `--json`:
//! one JSON document on standard output, errors included, described in
//! `docs/json-output.md`.

#[cfg(windows)]
mod demo;
#[cfg(windows)]
mod install;
mod output;
mod validate;

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
    after_help = "The commands that talk to the daemon, open to stop, need an administrator terminal."
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
    /// One scripted demo agent. `daifuku demo` starts these.
    #[command(hide = true)]
    DemoAgent {
        /// Which of the fleet's terminals this is, from 1.
        number: Option<usize>,
    },
    /// Show fleets, agents, hotkeys and the config in use.
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
    /// administrator rights when this terminal has none. Every key in it is
    /// optional.
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
    /// Install for this user: Program Files, the logon task, the hooks for
    /// Claude Code and Codex. Needs an administrator terminal.
    Install {
        /// Leave Claude Code's and Codex's settings alone.
        #[arg(long)]
        no_hooks: bool,
        /// Stop a running daemon and do not start it again; it starts at the
        /// next logon.
        #[arg(long)]
        no_start: bool,
        /// Wait for Enter at the end, so the window the offer to install
        /// opened stays until it is read.
        #[arg(long, hide = true)]
        pause: bool,
    },
    /// Remove the program, the logon task and Daifuku's hooks. Keeps the
    /// config, the logs and the saved state unless --purge. Needs an
    /// administrator terminal.
    Uninstall {
        /// Also delete the config, the logs and the saved state.
        #[arg(long)]
        purge: bool,
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
            no_hooks,
            no_start,
            pause,
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
        Command::Uninstall { purge } => setup_result(uninstall_cmd(purge)),
        Command::Doctor { format } => doctor_cmd(format.json),
        Command::Open { fleet, format } => control(&Request::Open { fleet }, format.json),
        Command::Snap { format } => control(&Request::Snap, format.json),
        Command::Close {
            all: true, format, ..
        } => control(&Request::CloseAll, format.json),
        Command::Close { fleet, format, .. } => control(&Request::Close { fleet }, format.json),
        Command::Next { format } => control(&Request::Next, format.json),
        Command::Demo { format } => control(&Request::Demo, format.json),
        Command::DemoAgent { number } => {
            demo_agent(number);
            ExitCode::SUCCESS
        }
        Command::Status { format } => control(&Request::Status, format.json),
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

/// The first line of clap's error, without its `error: ` in front.
fn usage_message(e: &clap::Error) -> String {
    let text = e.render().to_string();
    let first = text.lines().next().unwrap_or_default();
    first.strip_prefix("error: ").unwrap_or(first).to_owned()
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
const INSTALLED: &str = "\nDaifuku is installed and starts at every sign-in. In a new terminal, `daifuku doctor` checks the setup and `daifuku config` opens the settings.";

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
    if !exe.with_file_name("daifukud.exe").is_file() {
        println!("daifukud.exe is not in this folder, and Daifuku needs both programs.");
        println!(
            "If you opened daifuku.exe inside the zip, extract the zip first (right-click it, Extract All), then open daifuku.exe in the new folder."
        );
        wait_for_enter();
        return Some(ExitCode::FAILURE);
    }
    println!(
        "This installs Daifuku: it copies it to Program Files, starts it at every sign-in and adds its hooks to Claude Code and Codex."
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

/// Opens the config in the program Windows opens `.json` files with, or in
/// Notepad, and returns the file. Only administrators may change the config,
/// so from a terminal that is not elevated the editor starts after the
/// Windows prompt.
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
    let elevated = process::current_is_elevated();
    let notepad = paths::system_dir().map(|d| d.join("notepad.exe"));
    let quoted = format!("\"{}\"", file.display());
    let mut last = None;
    for editor in elevate::opens(".json").into_iter().chain(notepad) {
        let started = if elevated {
            elevate::run(&editor, &quoted)
        } else {
            elevate::run_elevated(&editor, &quoted)
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
    use daifuku_core::protocol::{Response, from_line, to_line};
    use daifuku_win::pipe::{Pipe, send};

    // Without --json a failure prints on stderr, as it always has.
    let fail = |code: ErrorCode, message: &str| {
        if json {
            fail_json(code, message)
        } else {
            eprintln!("daifuku: {message}");
            ExitCode::FAILURE
        }
    };
    let Ok(line) = to_line(request) else {
        return if json {
            fail_json(ErrorCode::Internal, "could not write the request")
        } else {
            ExitCode::FAILURE
        };
    };
    let reply = match send(Pipe::Control, &line, std::time::Duration::from_secs(2)) {
        Ok(Some(reply)) => reply,
        Ok(None) if json => return fail_json(ErrorCode::Ipc, "the daemon did not answer"),
        Ok(None) => return ExitCode::FAILURE,
        Err(e) => {
            let elevated = daifuku_win::process::current_is_elevated();
            let message = control_error(&e, elevated, &command_line(request, json));
            return fail(control_code(&e, elevated), &message);
        }
    };
    match from_line::<Response>(&reply) {
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
        Ok(Response::Error { message }) => fail(ErrorCode::Daemon, &message),
        Err(e) => fail(ErrorCode::Ipc, &format!("unreadable reply: {e}")),
    }
}

/// What to say when the daemon could not be asked. Only an elevated process
/// may open the control pipe, as a request can open administrator terminals,
/// so for any other process every failure but a missing daemon means Windows
/// refused it. Then the message ends with `command`, the command to run again
/// in an administrator terminal.
#[cfg_attr(not(windows), allow(dead_code))]
fn control_error(e: &std::io::Error, elevated: bool, command: &str) -> String {
    if !elevated && e.kind() != std::io::ErrorKind::NotFound {
        format!(
            "the daemon answers only an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: {command}"
        )
    } else {
        e.to_string()
    }
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
        Request::Snap => "snap".to_owned(),
        Request::Next => "next".to_owned(),
        Request::Demo => "demo".to_owned(),
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

/// The error code for a daemon that could not be asked, matching
/// [`control_error`].
#[cfg_attr(not(windows), allow(dead_code))]
fn control_code(e: &std::io::Error, elevated: bool) -> ErrorCode {
    if e.kind() == std::io::ErrorKind::NotFound {
        ErrorCode::DaemonNotRunning
    } else if !elevated {
        ErrorCode::NotElevated
    } else {
        ErrorCode::Ipc
    }
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
            "the daemon answers only an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status --json"
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
        let spoofed = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the process answering on Daifuku's control pipe is not the elevated daemon",
        );
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

    /// The `--json` flag of a command, `None` for one that has none.
    fn json_flag(command: &Command) -> Option<bool> {
        match command {
            Command::Open { format, .. }
            | Command::Snap { format }
            | Command::Close { format, .. }
            | Command::Next { format }
            | Command::Demo { format }
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
            Command::DemoAgent { .. }
            | Command::Hook
            | Command::Install { .. }
            | Command::Uninstall { .. } => None,
        }
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
            &["reload", "--json"],
            &["stop", "--json"],
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
        for args in [
            &["--json", "status"][..],
            &["install", "--json"],
            &["uninstall", "--json"],
            &["hook", "--json"],
        ] {
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
    }

    #[test]
    fn open_takes_an_optional_fleet() {
        let cli = Cli::parse_from(["daifuku", "open", "mochi"]);
        assert!(matches!(cli.command, Command::Open { fleet: Some(ref f), .. } if f == "mochi"));
        let cli = Cli::parse_from(["daifuku", "open"]);
        assert!(matches!(cli.command, Command::Open { fleet: None, .. }));
    }
}
