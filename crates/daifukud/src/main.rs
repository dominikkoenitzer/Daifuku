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
//! daifukud [--config <file>]
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

fn main() {
    #[cfg(windows)]
    {
        let config = config_arg(std::env::args().skip(1));
        if let Err(error) = daemon::run(config) {
            tracing::error!(error = %format!("{error:#}"), "daifukud stopped");
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("daifukud runs on Windows only");
        std::process::exit(1);
    }
}

/// The value of `--config`, if given.
#[cfg_attr(not(windows), allow(dead_code))]
fn config_arg(mut args: impl Iterator<Item = String>) -> Option<std::path::PathBuf> {
    while let Some(a) = args.next() {
        if a == "--config" {
            return args.next().map(Into::into);
        }
        if let Some(v) = a.strip_prefix("--config=") {
            return Some(v.into());
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
            config_arg(args(&["--config", "a.json"])),
            Some("a.json".into())
        );
        assert_eq!(
            config_arg(args(&["--config=b.json"])),
            Some("b.json".into())
        );
        assert_eq!(config_arg(args(&[])), None);
        assert_eq!(config_arg(args(&["--config"])), None);
    }
}
