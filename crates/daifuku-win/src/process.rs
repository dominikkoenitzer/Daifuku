//! Processes: who started whom, and who runs elevated.

use std::collections::HashMap;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, WAIT_OBJECT_0,
    WIN32_ERROR,
};
use windows::Win32::Security::{
    DuplicateTokenEx, EqualSid, GetTokenInformation, RevertToSelf, SecurityImpersonation,
    TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_IMPERSONATE, TOKEN_QUERY, TOKEN_USER, TokenElevation,
    TokenImpersonation, TokenPrimary, TokenUser,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT,
    CreateProcessW, CreateProcessWithTokenW, DETACHED_PROCESS, GetCurrentProcess,
    GetCurrentProcessId, GetProcessTimes, LOGON_WITH_PROFILE, NORMAL_PRIORITY_CLASS, OpenProcess,
    OpenProcessToken, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, STARTUPINFOW, SetThreadToken, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetShellWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_SHUTTINGDOWN,
};
use windows::core::{PCWSTR, PWSTR};

use crate::wide::from_wide;

/// A process watched until it ends. The handle keeps its id from being
/// handed to another process meanwhile.
pub struct Watch(HANDLE);

impl Watch {
    /// Watches process `pid`, or `None` when it cannot be opened, as when it
    /// has ended already.
    #[must_use]
    pub fn open(pid: u32) -> Option<Self> {
        // SAFETY: the handle is owned by the returned value.
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
            .ok()
            .map(Self)
    }

    /// Whether the process has ended.
    #[must_use]
    pub fn ended(&self) -> bool {
        // SAFETY: a valid handle, not waited on.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Whether the session is signing out or Windows is shutting down, when
/// windows close because everything stops, not because someone closed them.
#[must_use]
pub fn session_ending() -> bool {
    // SAFETY: no pointers.
    unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

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

/// When a process started, as a FILETIME tick count: 100 ns since 1601.
#[must_use]
pub fn started(pid: u32) -> Option<u64> {
    // SAFETY: the handle is closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let at = started_at(h);
        let _ = CloseHandle(h);
        at
    }
}

/// When the process behind `h` started, as [`started`] says it.
///
/// # Safety
///
/// `h` must be a process handle open for at least limited queries.
unsafe fn started_at(h: HANDLE) -> Option<u64> {
    let (mut c, mut e, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: as the caller promises; the FILETIMEs are valid out pointers.
    unsafe { GetProcessTimes(h, &raw mut c, &raw mut e, &raw mut k, &raw mut u) }.ok()?;
    Some((u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime))
}

/// Every process in `all` that one of `roots` started, or one of those, and
/// so on down, the roots first, each with when it started by `started`. A
/// process `spare` names is listed, but not what it started.
///
/// As in [`ancestors`], a child counts only if it started after its parent:
/// the parent id of a process whose parent ended may name a later one.
#[must_use]
pub fn descendants(
    all: &HashMap<u32, Proc>,
    started: impl Fn(u32) -> Option<u64>,
    spare: impl Fn(u32) -> bool,
    roots: &[u32],
) -> Vec<(Proc, u64)> {
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    for p in all.values() {
        if p.parent != p.pid {
            children.entry(p.parent).or_default().push(p);
        }
    }
    let mut out: Vec<(Proc, u64)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut at = 0;
    for &root in roots {
        if let Some(p) = all.get(&root)
            && let Some(when) = started(root)
            && seen.insert(root)
        {
            out.push((p.clone(), when));
        }
    }
    while at < out.len() {
        let (pid, when) = (out[at].0.pid, out[at].1);
        at += 1;
        if spare(pid) {
            continue;
        }
        let mut found: Vec<&Proc> = children.get(&pid).cloned().unwrap_or_default();
        found.sort_by_key(|c| c.pid);
        for child in found {
            if seen.contains(&child.pid) {
                continue;
            }
            if let Some(child_when) = started(child.pid)
                && child_when >= when
            {
                seen.insert(child.pid);
                out.push((child.clone(), child_when));
            }
        }
    }
    out
}

/// Whether process `pid` is still the one that started at `started` and has
/// not ended. Unlike [`started`], not fooled by a process that ended while
/// someone still holds a handle to it.
#[must_use]
pub fn runs(pid: u32, started: u64) -> bool {
    // SAFETY: the handle is closed below.
    unsafe {
        let Ok(h) = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        ) else {
            return false;
        };
        let runs = started_at(h) == Some(started) && WaitForSingleObject(h, 0) != WAIT_OBJECT_0;
        let _ = CloseHandle(h);
        runs
    }
}

/// Ends process `pid` if it is still the one that started at `started`, and
/// says whether it did: not when it ended already, or its id names a process
/// started since.
///
/// # Errors
///
/// When it runs and cannot be ended, as one that runs elevated cannot from a
/// process that does not.
pub fn end(pid: u32, started: u64) -> std::io::Result<bool> {
    use std::io::Error;

    // SAFETY: the handle is closed on every path.
    unsafe {
        let h = match OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        ) {
            Ok(h) => h,
            Err(e) if WIN32_ERROR::from_error(&e) == Some(ERROR_INVALID_PARAMETER) => {
                return Ok(false);
            }
            Err(e) => return Err(Error::other(e)),
        };
        let same = started_at(h) == Some(started);
        let running = WaitForSingleObject(h, 0) != WAIT_OBJECT_0;
        let ended = if same && running {
            TerminateProcess(h, 1).map(|()| true).map_err(Error::other)
        } else {
            Ok(false)
        };
        let _ = CloseHandle(h);
        ended
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

/// Whether another process runs as the same account as this one, `None` if
/// it cannot be asked.
#[must_use]
pub fn runs_as_this_user(pid: u32) -> Option<bool> {
    // SAFETY: every handle is closed before returning.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut theirs = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &raw mut theirs);
        let _ = CloseHandle(process);
        opened.ok()?;
        let mut own = HANDLE::default();
        let same = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut own)
            .is_ok()
            .then(|| {
                let same = same_user(theirs, own);
                let _ = CloseHandle(own);
                same
            });
        let _ = CloseHandle(theirs);
        same
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

/// The Remote Desktop session another process runs in, `None` when Windows
/// does not say. Needs no rights over the process beyond what any user has
/// over an elevated one of their own.
#[must_use]
pub fn session_of(pid: u32) -> Option<u32> {
    let mut id = 0u32;
    // SAFETY: id is a valid out pointer.
    unsafe { ProcessIdToSessionId(pid, &raw mut id) }.ok()?;
    Some(id)
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
    use std::io::Error;

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

    let token = shell_token()?;
    // SAFETY: the duplicate is closed by `Revert`, or here when it cannot be
    // put on the thread.
    unsafe {
        let mut user = HANDLE::default();
        DuplicateTokenEx(
            HANDLE(token.as_raw_handle()),
            TOKEN_ACCESS_MASK(TOKEN_IMPERSONATE.0 | TOKEN_QUERY.0),
            None,
            SecurityImpersonation,
            TokenImpersonation,
            &raw mut user,
        )
        .map_err(Error::other)?;
        drop(token);
        if let Err(e) = SetThreadToken(None, Some(user)) {
            let _ = CloseHandle(user);
            return Err(Error::other(e));
        }
        let _revert = Revert(user);
        Ok(f())
    }
}

/// The desktop shell's token, open to be duplicated and queried, when the
/// shell runs as the same account as this process.
///
/// # Errors
///
/// `NotFound` when there is no desktop shell, and `PermissionDenied` when it
/// runs as another account.
fn shell_token() -> std::io::Result<OwnedHandle> {
    use std::io::{Error, ErrorKind};

    // SAFETY: the process handle and this process's token are closed on
    // every path, the shell's token is owned by the value returned, and the
    // SID buffers outlive the comparison.
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
        let token = OwnedHandle::from_raw_handle(token.0);
        let mut own = HANDLE::default();
        let same = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut own).is_ok() && {
            let same = same_user(HANDLE(token.as_raw_handle()), own);
            let _ = CloseHandle(own);
            same
        };
        if !same {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "the desktop belongs to another account than this process",
            ));
        }
        Ok(token)
    }
}

/// Starts `exe`, with no arguments, as a background program of this
/// process's user with the user's own rights, one that runs on after
/// whatever started it has ended.
///
/// It takes nothing over from this process. No handles: an agent that reads
/// this program's output through a pipe would otherwise wait for the new
/// program to end as well. No working folder, which it would keep from being
/// deleted: it starts in its own. No environment: it gets the one Windows
/// builds for the user, without the agents' variables. And no low priority:
/// it runs at the normal one. From an elevated process it starts with the
/// desktop shell's token, so it never runs elevated only because the command
/// that started it did.
///
/// # Errors
///
/// When it cannot be started. From an elevated process also `NotFound` when
/// there is no desktop shell and `PermissionDenied` when the shell runs as
/// another account.
pub fn start_detached(exe: &Path) -> std::io::Result<()> {
    use std::io::Error;

    let app: Vec<u16> = exe.as_os_str().encode_wide().chain(once(0)).collect();
    // CreateProcess may write to the command line, so it gets a copy of its
    // own: the program's path in quotes.
    let mut line: Vec<u16> = once(u16::from(b'"'))
        .chain(exe.as_os_str().encode_wide())
        .chain([u16::from(b'"'), 0])
        .collect();
    let folder: Option<Vec<u16>> = exe
        .parent()
        .map(|d| d.as_os_str().encode_wide().chain(once(0)).collect());
    let folder = folder
        .as_ref()
        .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr()));
    let si = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(0),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    if current_is_elevated() {
        let token = shell_token()?;
        // SAFETY: the duplicate is owned and closed on every path; the
        // strings and the environment are NUL terminated and outlive the
        // calls.
        unsafe {
            let mut primary = HANDLE::default();
            DuplicateTokenEx(
                HANDLE(token.as_raw_handle()),
                TOKEN_ACCESS_MASK(
                    TOKEN_QUERY.0
                        | TOKEN_ASSIGN_PRIMARY.0
                        | TOKEN_DUPLICATE.0
                        | TOKEN_IMPERSONATE.0
                        | TOKEN_ADJUST_DEFAULT.0
                        | TOKEN_ADJUST_SESSIONID.0,
                ),
                None,
                SecurityImpersonation,
                TokenPrimary,
                &raw mut primary,
            )
            .map_err(Error::other)?;
            let primary = OwnedHandle::from_raw_handle(primary.0);
            let environment = crate::user_env::block_for(HANDLE(primary.as_raw_handle()));
            // The Secondary Logon service starts it, so it is in none of this
            // process's jobs and holds none of its handles.
            CreateProcessWithTokenW(
                HANDLE(primary.as_raw_handle()),
                LOGON_WITH_PROFILE,
                PCWSTR(app.as_ptr()),
                Some(PWSTR(line.as_mut_ptr())),
                CREATE_UNICODE_ENVIRONMENT | NORMAL_PRIORITY_CLASS,
                environment.as_ref().map(|b| b.as_ptr().cast()),
                folder,
                &raw const si,
                &raw mut pi,
            )
            .map_err(Error::other)?;
        }
    } else {
        let environment = crate::user_env::block(&crate::user_env::for_terminal());
        // No console, and no share in the Ctrl+C of the console it came from.
        let flags = DETACHED_PROCESS
            | CREATE_NEW_PROCESS_GROUP
            | CREATE_UNICODE_ENVIRONMENT
            | NORMAL_PRIORITY_CLASS;
        let mut start = |how| {
            // SAFETY: the strings and the environment are NUL terminated and
            // outlive the call; `false` passes none of this process's
            // handles on.
            unsafe {
                CreateProcessW(
                    PCWSTR(app.as_ptr()),
                    Some(PWSTR(line.as_mut_ptr())),
                    None,
                    None,
                    false,
                    how,
                    Some(environment.as_ptr().cast()),
                    folder,
                    &raw const si,
                    &raw mut pi,
                )
            }
        };
        // Out of any job the starter runs in, which may end its processes
        // with it. A job that allows no breaking away refuses that, and then
        // it starts inside the job.
        match start(flags | CREATE_BREAKAWAY_FROM_JOB) {
            Err(e) if WIN32_ERROR::from_error(&e) == Some(ERROR_ACCESS_DENIED) => start(flags),
            started => started,
        }
        .map_err(Error::other)?;
    }
    // SAFETY: both handles were just returned by the start and are not used
    // again.
    unsafe {
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
    }
    Ok(())
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
    fn this_process_runs_as_this_user_and_the_system_process_does_not() {
        assert_eq!(runs_as_this_user(current()), Some(true));
        // The System process (pid 4) runs as SYSTEM, which no test runs as;
        // an ordinary process may not even open its token.
        assert_ne!(runs_as_this_user(4), Some(true));
    }

    #[test]
    fn elevation_of_this_process_agrees_both_ways() {
        assert_eq!(is_elevated(current()), Some(current_is_elevated()));
    }

    fn proc(pid: u32, parent: u32) -> Proc {
        Proc {
            pid,
            parent,
            name: format!("p{pid}.exe"),
        }
    }

    #[test]
    fn descendants_follow_children_that_started_after_their_parents() {
        // 10 started 20, which started 30; 40 names 20 as its parent, yet
        // started before it, so 20's id was someone else's then. 50 is not
        // one of theirs at all.
        let all: HashMap<u32, Proc> = [
            proc(10, 1),
            proc(20, 10),
            proc(30, 20),
            proc(40, 20),
            proc(50, 1),
        ]
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
        let when = |pid: u32| match pid {
            10 => Some(100),
            20 => Some(200),
            30 => Some(300),
            40 => Some(150),
            50 => Some(50),
            _ => None,
        };
        let found: Vec<(u32, u64)> = descendants(&all, when, |_| false, &[10, 99])
            .into_iter()
            .map(|(p, at)| (p.pid, at))
            .collect();
        assert_eq!(found, [(10, 100), (20, 200), (30, 300)]);
    }

    #[test]
    fn descendants_list_each_process_once() {
        let all: HashMap<u32, Proc> = [proc(10, 1), proc(20, 10)]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        let found = descendants(&all, |_| Some(1), |_| false, &[10, 20, 10]);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn descendants_list_a_spared_process_but_nothing_it_started() {
        // 20 shows a window: it and all it started are for its owner.
        let all: HashMap<u32, Proc> = [proc(10, 1), proc(20, 10), proc(30, 20), proc(40, 10)]
            .into_iter()
            .map(|p| (p.pid, p))
            .collect();
        let found: Vec<u32> = descendants(&all, |_| Some(1), |pid| pid == 20, &[10])
            .into_iter()
            .map(|(p, _)| p.pid)
            .collect();
        assert_eq!(found, [10, 20, 40]);
    }

    #[test]
    fn end_ends_only_the_process_that_started_then() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let at = started(pid).unwrap();
        assert!(
            !end(pid, at + 1).unwrap(),
            "a later start is another process"
        );
        assert!(child.try_wait().unwrap().is_none());
        assert!(runs(pid, at));
        assert!(!runs(pid, at + 1));
        assert!(end(pid, at).unwrap());
        assert!(child.wait().is_ok());
        assert!(!end(pid, at).unwrap(), "it has ended already");
        // The child's handle is still open here, so its id still names it.
        assert!(!runs(pid, at), "it has ended, though its handle is held");
    }
}
