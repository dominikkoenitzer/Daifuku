//! Pure logic for Daifuku: nothing in this crate calls Win32, so all of it is
//! tested on any machine, desktop or not.
//!
//! - [`grid`] turns a count and a monitor into one rectangle per terminal.
//! - [`state`] turns a stream of agent hook events into a state per window.
//! - [`config`] is the config file and its schema.
//! - [`hotkey`] parses `ctrl + alt + return`.

pub mod config;
pub mod geometry;
pub mod grid;
pub mod hotkey;
pub mod state;

pub use geometry::Rect;
