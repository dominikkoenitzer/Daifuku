//! Pure logic for Daifuku: nothing in this crate calls Win32, so all of it is
//! tested on any machine, desktop or not.
//!
//! - [`grid`] turns a count and a monitor into one rectangle per terminal.
//! - [`state`] turns a stream of agent hook events into a state per window.
//! - [`config`] is the config file and its schema.
//! - [`hotkey`] parses `ctrl + alt + return`.
//! - [`protocol`] is what travels over the daemon's pipes.
//! - [`agents`] adds and removes Daifuku's hooks for Claude Code and Codex.
//! - [`task`] is the scheduled task that starts the daemon at logon.
//! - [`saved`] is what the daemon keeps across a restart.
//! - [`path_list`] adds a folder to a `PATH` value and takes it out again.

pub mod agents;
pub mod config;
pub mod geometry;
pub mod grid;
pub mod hotkey;
pub mod monitor;
pub mod path_list;
pub mod protocol;
pub mod saved;
pub mod state;
pub mod task;

pub use geometry::Rect;
