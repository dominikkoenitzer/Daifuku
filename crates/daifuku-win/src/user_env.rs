//! The environment a terminal is started with: the one Windows builds for
//! the user from the registry as it is now, without the variables an agent
//! set for itself (see [`daifuku_core::peer::without_agent_variables`]).

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use daifuku_core::peer::without_agent_variables;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The environment for a terminal this process starts as itself: fresh for
/// this process's user, or this process's own when Windows cannot build one,
/// and without the agents' variables either way.
#[must_use]
pub fn for_terminal() -> Vec<(OsString, OsString)> {
    without_agent_variables(fresh().unwrap_or_else(|_| std::env::vars_os().collect()))
}

/// The environment for a terminal started with `token`, a user's primary
/// token, as an environment block, or `None` when Windows cannot build one.
#[must_use]
pub fn block_for(token: HANDLE) -> Option<Vec<u16>> {
    fresh_for(token)
        .ok()
        .map(|vars| block(&without_agent_variables(vars)))
}
/// The environment Windows builds for the user `token` belongs to, from the
/// registry as it is now, as Explorer would hand it to a program started
/// from the Start menu: `PATH` as last saved, not the copy this process
/// took at its start.
///
/// # Errors
///
/// When Windows cannot build the block.
pub fn fresh_for(token: HANDLE) -> io::Result<Vec<(OsString, OsString)>> {
    let mut block: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: block receives a pointer Windows allocates; it is read up to
    // its terminating empty string and freed with the matching call.
    unsafe {
        CreateEnvironmentBlock(&raw mut block, Some(token), false).map_err(io::Error::other)?;
        let vars = parse_block(block.cast::<u16>());
        let _ = DestroyEnvironmentBlock(block);
        Ok(vars)
    }
}

/// The environment Windows builds for this process's own user.
///
/// # Errors
///
/// When the token cannot be opened or the block cannot be built.
pub fn fresh() -> io::Result<Vec<(OsString, OsString)>> {
    let mut token = HANDLE::default();
    // SAFETY: the token is closed before returning.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE,
            &raw mut token,
        )
        .map_err(io::Error::other)?;
        let vars = fresh_for(token);
        let _ = CloseHandle(token);
        vars
    }
}

/// The variables of an environment block: `NAME=value` strings, each ended
/// by a NUL, the whole ended by an empty one. Names that start with `=`,
/// the command prompt's per-drive folders, are kept with their `=`.
///
/// # Safety
///
/// `block` points at a well-formed block.
unsafe fn parse_block(block: *const u16) -> Vec<(OsString, OsString)> {
    let mut vars = Vec::new();
    let mut at = block;
    loop {
        let mut len = 0;
        // SAFETY: the block ends with an empty string, so every read stays
        // inside it.
        while unsafe { *at.add(len) } != 0 {
            len += 1;
        }
        if len == 0 {
            return vars;
        }
        // SAFETY: `len` units were just read.
        let entry = unsafe { std::slice::from_raw_parts(at, len) };
        if let Some((name, value)) = split_entry(entry) {
            vars.push((OsString::from_wide(name), OsString::from_wide(value)));
        }
        // SAFETY: past the entry's NUL, still inside the block.
        at = unsafe { at.add(len + 1) };
    }
}

/// `NAME=value` split at the first `=` after the first character.
fn split_entry(entry: &[u16]) -> Option<(&[u16], &[u16])> {
    let eq = entry.iter().skip(1).position(|&c| c == u16::from(b'='))? + 1;
    Some((&entry[..eq], &entry[eq + 1..]))
}

/// An environment block for `CreateProcess`: `NAME=value` strings, each
/// ended by a NUL, the whole ended by one more.
#[must_use]
pub fn block(vars: &[(OsString, OsString)]) -> Vec<u16> {
    let mut out = Vec::new();
    for (name, value) in vars {
        out.extend(name.encode_wide());
        out.push(u16::from(b'='));
        out.extend(value.encode_wide());
        out.push(0);
    }
    if out.is_empty() {
        out.push(0);
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_reads_back_as_the_variables_it_was_made_of() {
        let vars = vec![
            (OsString::from("PATH"), OsString::from(r"C:\Windows;C:\bin")),
            (OsString::from("=C:"), OsString::from(r"C:\src")),
            (OsString::from("EMPTY"), OsString::new()),
            (OsString::from("A"), OsString::from("x=y")),
        ];
        let b = block(&vars);
        assert_eq!(&b[b.len() - 2..], &[0, 0]);
        // SAFETY: a block just made, ended by an empty string.
        assert_eq!(unsafe { parse_block(b.as_ptr()) }, vars);
        assert_eq!(block(&[]), vec![0, 0]);
    }

    #[test]
    fn the_fresh_environment_has_the_user_s_folders_and_no_agent_variables() {
        let vars = for_terminal();
        let has = |name: &str| vars.iter().any(|(n, _)| n.eq_ignore_ascii_case(name));
        assert!(has("PATH") && has("SystemRoot"), "{vars:?}");
        assert!(
            vars.iter()
                .all(|(n, _)| !daifuku_core::peer::is_agent_variable(n)),
            "{vars:?}"
        );
    }
}
