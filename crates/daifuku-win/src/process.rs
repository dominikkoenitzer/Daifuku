//! Processes: who started whom, and who runs elevated.

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::wide::from_wide;

/// One process from a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// Its id.
    pub pid: u32,
    /// The id of the process that started it, which may since have exited and
    /// had its id handed to someone else.
    pub parent: u32,
    /// The executable's file name, `claude.exe`.
    pub name: String,
}

/// Every running process, by id.
#[must_use]
pub fn snapshot() -> HashMap<u32, Proc> {
    let mut out = HashMap::new();
    // SAFETY: the snapshot handle is closed below; the entry has its size set.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0),
            ..Default::default()
        };
        let mut more = Process32FirstW(snap, &raw mut e).is_ok();
        while more {
            out.insert(
                e.th32ProcessID,
                Proc {
                    pid: e.th32ProcessID,
                    parent: e.th32ParentProcessID,
                    name: from_wide(&e.szExeFile),
                },
            );
            more = Process32NextW(snap, &raw mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// The chain of processes from `pid` up to the root, `pid` first.
///
/// A parent id is only trusted if that process started before its child:
/// ids are reused, and a parent that exited may have handed its id to a
/// process that started later and has nothing to do with this one.
#[must_use]
pub fn ancestors(pid: u32) -> Vec<Proc> {
    let all = snapshot();
    let mut chain = Vec::new();
    let mut at = pid;
    while let Some(p) = all.get(&at) {
        if chain.iter().any(|c: &Proc| c.pid == p.pid) || chain.len() > 64 {
            break;
        }
        chain.push(p.clone());
        let parent = p.parent;
        if parent == 0 || parent == at {
            break;
        }
        match (started(parent), started(at)) {
            (Some(parent_start), Some(child_start)) if parent_start <= child_start => at = parent,
            _ => break,
        }
    }
    chain
}

/// When a process started, as a FILETIME tick count.
fn started(pid: u32) -> Option<u64> {
    // SAFETY: the handle is closed before returning; the FILETIMEs are
    // valid out pointers.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let ok = GetProcessTimes(h, &raw mut c, &raw mut e, &raw mut k, &raw mut u);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some((u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime))
    }
}

/// Whether this process runs elevated.
#[must_use]
pub fn current_is_elevated() -> bool {
    // SAFETY: the pseudo handle needs no closing.
    token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}

/// Whether another process runs elevated, `None` if it cannot be asked.
#[must_use]
pub fn is_elevated(pid: u32) -> Option<bool> {
    // SAFETY: the handle is closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let r = token_elevated(h);
        let _ = CloseHandle(h);
        r
    }
}

fn token_elevated(process: HANDLE) -> Option<bool> {
    // SAFETY: the token is closed before returning; the buffer is a
    // TOKEN_ELEVATION of the size passed.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &raw mut token).ok()?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(0),
            &raw mut len,
        );
        let _ = CloseHandle(token);
        ok.ok()?;
        Some(elevation.TokenIsElevated != 0)
    }
}

/// The id of this process.
#[must_use]
pub fn current() -> u32 {
    // SAFETY: no arguments.
    unsafe { GetCurrentProcessId() }
}

/// The Remote Desktop session this process runs in, which names the pipes so
/// two people signed in at once each get their own daemon.
#[must_use]
pub fn session() -> u32 {
    let mut id = 0u32;
    // SAFETY: id is a valid out pointer.
    let _ = unsafe { ProcessIdToSessionId(current(), &raw mut id) };
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_the_first_of_its_own_ancestors() {
        let chain = ancestors(current());
        assert!(!chain.is_empty());
        assert_eq!(chain[0].pid, current());
        // Every parent really started before its child.
        for pair in chain.windows(2) {
            assert_eq!(pair[0].parent, pair[1].pid);
        }
    }

    #[test]
    fn an_unknown_pid_has_no_ancestors() {
        assert!(ancestors(u32::MAX - 3).is_empty());
    }

    #[test]
    fn elevation_of_this_process_agrees_both_ways() {
        assert_eq!(is_elevated(current()), Some(current_is_elevated()));
    }
}
