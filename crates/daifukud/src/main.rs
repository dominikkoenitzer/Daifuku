//! The Daifuku daemon.
//!
//! One thread owns all state and runs a Win32 message loop. Everything that
//! happens arrives there as a message: hotkeys as `WM_HOTKEY`, window events
//! from `SetWinEventHook`, a periodic `WM_TIMER`, and whatever the two pipe
//! threads received, which they hand over through a channel and wake the loop
//! for with a posted message. Nothing else touches the state, so nothing needs
//! a lock.
//!
//! ```text
//! daifukud [--config <file>] [--state <folder>]
//! daifukud --version
//! ```

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod daemon;
#[cfg(windows)]
mod events;
#[cfg(windows)]
mod fleet;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod logging;
#[cfg(all(windows, test))]
mod test_support;

fn main() {
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        // Printed even though the daemon has no console: a release check or
        // a person piping it still reads it.
        println!("daifukud {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    #[cfg(windows)]
    {
        let config = value_of("--config", std::env::args().skip(1)).map(Into::into);
        let state = value_of("--state", std::env::args().skip(1)).map(Into::into);
        // The log outlives `run`, so a failure to start is written down
        // before the writer stops.
        let elevated = daifuku_win::process::current_is_elevated();
        let log = logging::init(daifuku_win::paths::log_dir(elevated).as_deref());
        if let Err(error) = daemon::run(config, state) {
            tracing::error!(error = %format!("{error:#}"), "daifukud stopped");
            drop(log);
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("daifukud runs on Windows only");
        std::process::exit(1);
    }
}

/// The value of `flag`, as `--config a.json` or `--config=a.json`, if given.
#[cfg_attr(not(windows), allow(dead_code))]
fn value_of(flag: &str, mut args: impl Iterator<Item = String>) -> Option<String> {
    let joined = format!("{flag}=");
    while let Some(a) = args.next() {
        if a == flag {
            return args.next();
        }
        if let Some(v) = a.strip_prefix(&joined) {
            return Some(v.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn reads_the_config_flag_both_ways() {
        assert_eq!(
            value_of("--config", args(&["--config", "a.json"])).as_deref(),
            Some("a.json")
        );
        assert_eq!(
            value_of("--config", args(&["--config=b.json"])).as_deref(),
            Some("b.json")
        );
        assert_eq!(value_of("--config", args(&[])), None);
        assert_eq!(value_of("--config", args(&["--config"])), None);
        assert_eq!(
            value_of("--state", args(&["--config", "a.json", "--state", "s"])).as_deref(),
            Some("s")
        );
    }
}
