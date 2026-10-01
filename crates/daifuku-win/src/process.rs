//! Processes: who started whom, and who runs elevated.

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::Security::{
    DuplicateTokenEx, EqualSid, GetTokenInformation, RevertToSelf, SecurityImpersonation,
    TOKEN_ACCESS_MASK, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_IMPERSONATE, TOKEN_QUERY,
    TOKEN_USER, TokenElevation, TokenImpersonation, TokenUser,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, SetThreadToken,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

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

/// Whether the desktop shell runs elevated, as it does with UAC turned off,
/// `None` when there is no shell or it cannot be asked. A program started
/// with the shell's rights runs the same way.
#[must_use]
pub fn shell_is_elevated() -> Option<bool> {
    // SAFETY: plain calls; the pid is read into a local.
    let pid = unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&raw mut pid));
        pid
    };
    is_elevated(pid)
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

/// Runs `f` on this thread as the user of the desktop shell, with that
/// user's ordinary rights, and goes back to this process's own rights after.
///
/// An elevated process that writes into the user's profile can be steered:
/// the profile is the user's to change, and a link swapped in at the right
/// moment points the write at a file only administrators may change. Written
/// as the user, such a file is refused like any other.
///
/// # Errors
///
/// `NotFound` when there is no desktop shell to borrow rights from, and
/// `PermissionDenied` when the shell runs as another account than this
/// process, as it does when an administrator's password was typed into a
/// standard user's prompt. `f` does not run then.
pub fn as_shell_user<T>(f: impl FnOnce() -> T) -> std::io::Result<T> {
    use std::io::{Error, ErrorKind};

    /// Goes back to the process's own rights, also when `f` panics.
    struct Revert(HANDLE);
    impl Drop for Revert {
        fn drop(&mut self) {
            // SAFETY: undoes the SetThreadToken below and closes its token.
            unsafe {
                let _ = RevertToSelf();
                let _ = CloseHandle(self.0);
            }
        }
    }

    // SAFETY: every handle is closed on every path; the SID buffers outlive
    // the comparison.
    unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return Err(Error::new(ErrorKind::NotFound, "no desktop shell"));
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&raw mut pid));
        let process =
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(Error::other)?;
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(
            process,
            TOKEN_ACCESS_MASK(TOKEN_DUPLICATE.0 | TOKEN_QUERY.0),
            &raw mut token,
        );
        let _ = CloseHandle(process);
        opened.map_err(Error::other)?;
        let mut own = HANDLE::default();
        let same = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut own).is_ok() && {
            let same = same_user(token, own);
            let _ = CloseHandle(own);
            same
        };
        if !same {
            let _ = CloseHandle(token);
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "the desktop belongs to another account than this process",
            ));
        }
        let mut user = HANDLE::default();
        let duplicated = DuplicateTokenEx(
            token,
            TOKEN_ACCESS_MASK(TOKEN_IMPERSONATE.0 | TOKEN_QUERY.0),
            None,
            SecurityImpersonation,
            TokenImpersonation,
            &raw mut user,
        );
        let _ = CloseHandle(token);
        duplicated.map_err(Error::other)?;
        if let Err(e) = SetThreadToken(None, Some(user)) {
            let _ = CloseHandle(user);
            return Err(Error::other(e));
        }
        let _revert = Revert(user);
        Ok(f())
    }
}

/// Whether two tokens belong to the same account.
///
/// # Safety
///
/// Both handles must be tokens open for `TOKEN_QUERY`.
unsafe fn same_user(a: HANDLE, b: HANDLE) -> bool {
    /// The token's user, in a buffer that starts with a `TOKEN_USER`.
    unsafe fn user(token: HANDLE) -> Option<Vec<u64>> {
        let mut len = 0u32;
        // SAFETY: the first call only reports the size.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &raw mut len) };
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: the buffer holds `len` bytes, aligned for TOKEN_USER.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                len,
                &raw mut len,
            )
        }
        .ok()?;
        Some(buf)
    }
    // SAFETY: as the caller promises; both SIDs point into live buffers.
    unsafe {
        let (Some(x), Some(y)) = (user(a), user(b)) else {
            return false;
        };
        let x = &*x.as_ptr().cast::<TOKEN_USER>();
        let y = &*y.as_ptr().cast::<TOKEN_USER>();
        EqualSid(x.User.Sid, y.User.Sid).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether this thread's own token, when it has one, is elevated.
    fn thread_elevated() -> bool {
        use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};
        // SAFETY: the token is closed before returning; the out buffer has
        // the size passed.
        unsafe {
            let mut token = HANDLE::default();
            if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &raw mut token).is_err() {
                return false;
            }
            let mut e = TOKEN_ELEVATION::default();
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some((&raw mut e).cast()),
                u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(0),
                &raw mut len,
            );
            let _ = CloseHandle(token);
            ok.is_ok() && e.TokenIsElevated != 0
        }
    }

    #[test]
    fn a_write_as_the_shell_user_has_only_that_users_rights() {
        let Some(program_files) =
            crate::paths::install_dir().and_then(|d| d.parent().map(std::path::Path::to_path_buf))
        else {
            return;
        };
        let probe = program_files.join(format!("daifuku-probe-{}", std::process::id()));
        // No desktop shell, or one of another account: nothing to prove.
        let Ok((elevated, written)) =
            as_shell_user(|| (thread_elevated(), std::fs::write(&probe, "x")))
        else {
            return;
        };
        let _ = std::fs::remove_file(&probe);
        // With UAC off the shell itself is elevated and may write there.
        if !elevated {
            assert!(
                written.is_err(),
                "a write as the user reached Program Files"
            );
        }
        assert!(
            !thread_elevated() || current_is_elevated(),
            "the rights were not given back"
        );
    }

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
