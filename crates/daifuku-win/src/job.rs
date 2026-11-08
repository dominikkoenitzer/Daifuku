//! Job objects: a process tree that runs at one priority and ends as one.
//!
//! `daifuku group` puts itself in a new job and starts its command there, so
//! every process the command starts, and every one those start, is in it.
//! The job holds them all at its priority class, which none of them can
//! raise, and Windows ends all of them when the job's last handle closes:
//! when `daifuku group` exits, or its terminal closes. A shell or a watcher an
//! agent left behind goes with it. The job is named after the group's
//! process, so the daemon can ask it what still runs in the terminal.

use std::io;

use daifuku_core::config::Priority;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_MORE_DATA, GetLastError, HANDLE, SetLastError,
    WAIT_TIMEOUT, WIN32_ERROR,
};

use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PRIORITY_CLASS, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, OpenJobObjectW, QueryInformationJobObject,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CreateMutexW, GetCurrentProcess, IDLE_PRIORITY_CLASS, INFINITE,
    NORMAL_PRIORITY_CLASS, OpenProcess, PROCESS_SYNCHRONIZE, ReleaseMutex, WaitForSingleObject,
};
use windows::core::{BOOL, PCWSTR};

use crate::wide::to_wide;

/// The name of the lock heavy jobs take turns at, one per signed-in session,
/// unless `DAIFUKU_HEAVY_QUEUE` names another, as the tests do so they never
/// wait for a real build.
const HEAVY: &str = r"Local\Daifuku.Heavy";

/// Set for everything a heavy job runs: a heavy job started from inside one
/// has its turn already, and waiting for the turn its own parent holds would
/// never end.
pub const IN_HEAVY: &str = "DAIFUKU_HEAVY";

/// How long the group waits for its program when the console closes, in
/// milliseconds. Windows gives every process five seconds then; the program
/// gets most of them for its own shutdown before the group ends it.
const CLOSE_GRACE_MS: u32 = 4000;

/// The program the group runs, once it started.
static CHILD: AtomicU32 = AtomicU32::new(0);

/// The right to list a job's processes.
const JOB_OBJECT_QUERY: u32 = 0x0004;

/// The most processes [`members`] lists of one job.
const MAX_MEMBERS: usize = 4096;

/// The name of the job the process `pid` made, as `daifuku group` does.
fn name_for(pid: u32) -> String {
    format!(r"Local\Daifuku.Group.{pid}")
}

/// A job object, closed when dropped, which ends every process in it.
pub struct Job(HANDLE);

impl Job {
    /// A new job whose processes run at `priority` and end with it.
    ///
    /// # Errors
    ///
    /// When Windows cannot make the job or set its limits.
    pub fn new(priority: Priority) -> io::Result<Self> {
        let name = to_wide(&name_for(std::process::id()));
        // SAFETY: the name is NUL terminated and there are no security
        // attributes; the handle is owned by the returned value. A job of
        // that name already there is another's, so this one goes unnamed.
        let job = unsafe {
            SetLastError(WIN32_ERROR(0));
            let named = CreateJobObjectW(None, PCWSTR(name.as_ptr())).map_err(io::Error::other)?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(named);
                CreateJobObjectW(None, None).map_err(io::Error::other)?
            } else {
                named
            }
        };
        let job = Self(job);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PRIORITY_CLASS;
        limits.BasicLimitInformation.PriorityClass = match priority {
            Priority::Idle => IDLE_PRIORITY_CLASS.0,
            Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS.0,
            Priority::Normal => NORMAL_PRIORITY_CLASS.0,
        };
        // SAFETY: the pointer and size describe `limits`, which outlives the
        // call.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
            )
        }
        .map_err(io::Error::other)?;
        Ok(job)
    }

    /// Puts this process in the job, so every process it starts from now on
    /// is in it too.
    ///
    /// # Errors
    ///
    /// When Windows refuses, as it does for a process whose own job allows
    /// no other.
    pub fn add_this_process(&self) -> io::Result<()> {
        // SAFETY: both handles are valid; the process one is a pseudo
        // handle that needs no closing.
        unsafe { AssignProcessToJobObject(self.0, GetCurrentProcess()) }.map_err(io::Error::other)
    }
}

/// The processes that run in the job `daifuku group` with process id `pid`
/// made: everything its terminal runs, down to a process whose parent ended,
/// short of one that left the job. `None` when there is no such job, as for
/// a terminal with no group or one an older version opened, or it cannot be
/// read.
#[must_use]
pub fn members(pid: u32) -> Option<Vec<u32>> {
    let name = to_wide(&name_for(pid));
    // SAFETY: the name is NUL terminated; the handle is owned by `job`.
    let job = Job(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, false, PCWSTR(name.as_ptr())) }.ok()?);
    let start = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList)
        .div_ceil(size_of::<usize>());
    let mut room = 64;
    loop {
        let mut list = vec![0usize; start + room];
        // SAFETY: the buffer is as long as the size given, and aligned for
        // the list's fields.
        let asked = unsafe {
            QueryInformationJobObject(
                Some(job.0),
                JobObjectBasicProcessIdList,
                list.as_mut_ptr().cast(),
                u32::try_from(list.len() * size_of::<usize>()).ok()?,
                None,
            )
        };
        match asked {
            Ok(()) => {
                // SAFETY: the buffer starts with the list's header.
                let listed = unsafe {
                    (*list.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>())
                        .NumberOfProcessIdsInList
                };
                let listed = usize::try_from(listed).ok()?.min(room);
                return Some(
                    list[start..start + listed]
                        .iter()
                        .filter_map(|&p| u32::try_from(p).ok())
                        .collect(),
                );
            }
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() && room < MAX_MEMBERS => {
                room *= 8;
            }
            Err(_) => return None,
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Keeps this process alive through Ctrl+C and Ctrl+Break, which the
/// console sends to every process in it: they are meant for the program
/// running in the group, and the group ending would end that program too.
/// When the console closes, the group waits up to four seconds for the
/// program, which is told too, so an agent's own shutdown and its last hooks
/// run before the group ends what is left.
pub fn outlive_ctrl_c() {
    unsafe extern "system" fn handler(event: u32) -> BOOL {
        if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
            return BOOL::from(true);
        }
        let pid = CHILD.load(Ordering::SeqCst);
        // SAFETY: the handle is closed before returning.
        unsafe {
            if pid != 0
                && let Ok(child) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid)
            {
                let _ = WaitForSingleObject(child, CLOSE_GRACE_MS);
                let _ = CloseHandle(child);
            }
        }
        BOOL::from(false)
    }
    // SAFETY: the handler is a plain function that lives as long as the
    // process. A handler is not passed on to child processes, so they keep
    // their own Ctrl+C.
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(handler), true);
    }
}

/// Names `pid` as the program the group runs, for the wait when the console
/// closes.
pub fn running(pid: u32) {
    CHILD.store(pid, Ordering::SeqCst);
}

/// This process's turn at the heavy-job queue, until it is dropped or the
/// process exits.
pub struct Turn(HANDLE);

/// Waits until no other heavy job runs in this session, calling `waiting`
/// once first when one does.
///
/// # Errors
///
/// When Windows refuses the lock.
pub fn wait_turn(waiting: impl FnOnce()) -> io::Result<Turn> {
    let name = std::env::var("DAIFUKU_HEAVY_QUEUE").unwrap_or_else(|_| HEAVY.to_owned());
    let turn = Turn(queue(&name)?);
    // SAFETY: a valid handle. An abandoned lock, left by a job that was
    // killed, is ours as well.
    unsafe {
        if WaitForSingleObject(turn.0, 0) == WAIT_TIMEOUT {
            waiting();
            let _ = WaitForSingleObject(turn.0, INFINITE);
        }
    }
    Ok(turn)
}

/// The lock of the heavy-job queue called `name`, made or opened.
///
/// Elevated and ordinary terminals of one user share it with the defaults:
/// an elevated maker's lock admits the sign-in's logon SID, which the
/// user's ordinary processes carry too.
fn queue(name: &str) -> io::Result<HANDLE> {
    let name = to_wide(name);
    // SAFETY: the name is NUL terminated; the caller owns the handle.
    unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.map_err(io::Error::other)
}

impl Drop for Turn {
    fn drop(&mut self) {
        // SAFETY: the handle is owned, held by this thread, and closed once.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held by each test that makes a group's job, since this process has
    /// one name for it.
    static NAMED: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_group_s_job_lists_what_runs_in_it_and_a_second_one_goes_unnamed() {
        use std::os::windows::io::AsRawHandle;
        let _named = NAMED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let job = Job::new(Priority::Normal).unwrap();
        let again = Job::new(Priority::Normal).unwrap();
        assert_eq!(members(std::process::id()), Some(Vec::new()));
        let mut ping = std::process::Command::new("ping.exe")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // SAFETY: both handles are valid while `job` and `ping` live.
        unsafe { AssignProcessToJobObject(job.0, HANDLE(ping.as_raw_handle())) }.unwrap();
        assert_eq!(members(std::process::id()), Some(vec![ping.id()]));
        drop(again);
        assert_eq!(members(std::process::id()), Some(vec![ping.id()]));
        // The job ends what runs in it as it closes.
        drop(job);
        let _ = ping.wait();
        assert_eq!(members(std::process::id()), None);
        assert_eq!(members(u32::MAX), None);
    }

    #[test]
    fn an_ordinary_terminal_waits_at_a_queue_an_elevated_one_made() {
        let name = format!(r"Local\Daifuku.Heavy.ordinary-{}", std::process::id());
        // Made and held by this process, elevated on CI and in an
        // administrator terminal.
        let held = queue(&name).unwrap();
        // SAFETY: a valid handle.
        assert_ne!(unsafe { WaitForSingleObject(held, 0) }, WAIT_TIMEOUT);
        let (asked, released) = (
            std::sync::mpsc::channel::<bool>(),
            std::sync::mpsc::channel::<()>(),
        );
        // On another thread, with an ordinary terminal's rights: the
        // desktop shell's own where it runs ordinary beside this one, else
        // this process's without administrator rights, at medium integrity.
        let other = std::thread::spawn(move || {
            let ordinary_shell = crate::process::current_is_elevated()
                && crate::process::shell_is_elevated() == Some(false);
            let theirs = if ordinary_shell {
                crate::process::as_shell_user(|| queue(&name)).unwrap()
            } else {
                crate::test_support::as_ordinary_user(|| queue(&name))
            }
            .expect("an ordinary terminal could not open the queue");
            // SAFETY: a valid handle, closed once.
            unsafe {
                asked
                    .0
                    .send(WaitForSingleObject(theirs, 0) == WAIT_TIMEOUT)
                    .unwrap();
                released.1.recv().unwrap();
                let turn = WaitForSingleObject(theirs, 5000) != WAIT_TIMEOUT;
                let let_go = ReleaseMutex(theirs).is_ok();
                let _ = CloseHandle(theirs);
                (turn, let_go)
            }
        });
        assert!(
            asked.1.recv().unwrap(),
            "the ordinary terminal's turn came while the elevated job held the queue"
        );
        // SAFETY: the handle this thread holds, closed once.
        unsafe {
            let _ = ReleaseMutex(held);
            let _ = CloseHandle(held);
        }
        released.0.send(()).unwrap();
        assert_eq!(
            other.join().unwrap(),
            (true, true),
            "its turn, and letting go of it"
        );
    }

    #[test]
    fn a_job_takes_every_priority() {
        let _named = NAMED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (p, class) in [
            (Priority::Idle, IDLE_PRIORITY_CLASS),
            (Priority::BelowNormal, BELOW_NORMAL_PRIORITY_CLASS),
            (Priority::Normal, NORMAL_PRIORITY_CLASS),
        ] {
            let job = Job::new(p).unwrap();
            // Read from the job, since a group the suite runs in caps what
            // its processes show.
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            // SAFETY: the pointer and size describe `limits`, which outlives
            // the call.
            unsafe {
                QueryInformationJobObject(
                    Some(job.0),
                    JobObjectExtendedLimitInformation,
                    (&raw mut limits).cast(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap(),
                    None,
                )
            }
            .unwrap();
            assert_eq!(limits.BasicLimitInformation.PriorityClass, class.0, "{p:?}");
        }
    }
}
