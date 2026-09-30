//! Named pipes, one JSON line each way.
//!
//! Daifuku has two, and their security is the point of having two:
//!
//! | pipe | who may connect | why |
//! |---|---|---|
//! | hook | the signed-in user, at medium integrity or above | a hook runs at its agent's level, the medium label keeps low-integrity processes out, and the most a hook can do is colour a border |
//! | control | Administrators, elevated | a request can open administrator terminals |
//!
//! A pipe created by an elevated process carries a high mandatory label, and
//! Windows' no-write-up rule then stops every ordinary process from writing to
//! it whatever the access list says. The hook pipe therefore sets a medium
//! label explicitly; the control pipe keeps the high one.
//!
//! Both are created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a process that
//! grabbed the name first makes the daemon fail loudly instead of sharing it,
//! and the control client checks that whoever answers is elevated before it
//! sends anything.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_CONNECTED, ERROR_SEM_TIMEOUT, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    HLOCAL, INVALID_HANDLE_VALUE, LocalFree, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE,
    FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::PCWSTR;

use crate::process;
use crate::wide::to_wide;

/// Which of the two pipes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipe {
    /// Agent hooks report here.
    Hook,
    /// The command line sends requests here.
    Control,
}

/// The longest line either side accepts. A status reply for sixteen fleets of
/// sixteen terminals is a few kilobytes; anything past this is not Daifuku.
const MAX_LINE: usize = 256 * 1024;

impl Pipe {
    /// The pipe's full name, per Remote Desktop session so two people signed
    /// in at once each talk to their own daemon.
    #[must_use]
    pub fn name(self) -> String {
        let kind = match self {
            Self::Hook => "hook",
            Self::Control => "control",
        };
        format!(r"\\.\pipe\daifuku-{}-{kind}", process::session())
    }

    /// The security descriptor, in SDDL, for a daemon that runs `elevated`
    /// or not.
    ///
    /// - Hook: full access for SYSTEM and Administrators, read and write for
    ///   the signed-in user alone, by their SID (another person signed in at
    ///   the same time is not let in), and a medium no-write-up label. For a
    ///   pipe,
    ///   generic write includes the right to create server instances, so the
    ///   elevated daemon grants the user `FILE_GENERIC_READ` and
    ///   `FILE_WRITE_DATA` only (`0x12008b`): otherwise any of the user's
    ///   processes could add a server of its own and collect hook lines. A
    ///   daemon that is not elevated keeps generic write, since it creates
    ///   its further instances as that same user.
    /// - Control: full access for SYSTEM and Administrators only, high label.
    fn sddl(self, elevated: bool) -> String {
        // Without a SID, which only a broken token gives, no user entry at
        // all; `instances` refuses to create a hook pipe like that.
        let user = |rights: &str| {
            crate::setup::user_sid()
                .map(|sid| format!("(A;;{rights};;;{sid})"))
                .unwrap_or_default()
        };
        match (self, elevated) {
            (Self::Hook, true) => format!(
                "D:P(A;;GA;;;SY)(A;;GA;;;BA){}S:(ML;;NW;;;ME)",
                user("0x12008b")
            ),
            (Self::Hook, false) => {
                format!("D:P(A;;GA;;;SY)(A;;GA;;;BA){}S:(ML;;NW;;;ME)", user("GRGW"))
            }
            (Self::Control, _) => "D:P(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;HI)".to_owned(),
        }
    }
}

/// One server instance of a pipe. The daemon creates a few per pipe, one
/// thread each, and every thread loops on [`Instance::accept`].
///
/// An instance is created once and reused for every client: a client is
/// disconnected, not the instance destroyed. So from the moment the name is
/// claimed until the daemon exits, the name is held, and there is never a
/// gap in which another process could take it. With several instances,
/// several hooks firing in the same instant each find one free.
///
/// Every read and write on an instance has a deadline, so a client that
/// connects and then says nothing, or never reads its reply, holds the
/// instance for a moment and not for ever.
pub struct Instance {
    handle: OwnedHandle,
    /// Signalled when an operation on the handle completes.
    event: OwnedHandle,
}

/// One connected client. Dropping it disconnects the client and frees the
/// instance for the next one.
pub struct Connection<'a> {
    instance: &'a Instance,
    accepted: Instant,
    /// The client's process id, as Windows reports it.
    pub client: u32,
}

/// Claims the pipe name and creates `count` instances of it. Fails if any
/// other process already holds the name.
///
/// `count` is also the most instances the pipe may ever have, so once they
/// are all created no other process can add one.
///
/// # Errors
///
/// When the name is taken or the security descriptor is refused, and for
/// the hook pipe when this process cannot read its user's SID: every hook
/// would be turned away without a word.
pub fn instances(pipe: Pipe, count: usize) -> io::Result<Vec<Instance>> {
    if pipe == Pipe::Hook && crate::setup::user_sid().is_none() {
        return Err(io::Error::other(
            "cannot read this user's SID to let their hooks in",
        ));
    }
    instances_at(
        &pipe.name(),
        &pipe.sddl(process::current_is_elevated()),
        count,
    )
}

fn instances_at(name: &str, sddl: &str, count: usize) -> io::Result<Vec<Instance>> {
    let count = count.clamp(1, 254);
    let max = u32::try_from(count).unwrap_or(1);
    let mut all = Vec::with_capacity(count);
    for i in 0..count {
        all.push(Instance {
            handle: create(name, sddl, i == 0, max)?,
            event: event()?,
        });
    }
    Ok(all)
}

impl Instance {
    /// Waits for the next client on this instance.
    ///
    /// # Errors
    ///
    /// When the connection fails.
    pub fn accept(&mut self) -> io::Result<Connection<'_>> {
        let raw_handle = raw(&self.handle);
        // A client may connect before this call. ERROR_PIPE_CONNECTED says so,
        // and ERROR_NO_DATA says it has also hung up already, which a hook
        // does the moment it has written; what it wrote can still be read.
        // SAFETY: a valid pipe handle; overlapped() waits for the connect.
        let connected = overlapped(raw_handle, raw(&self.event), None, |ov| unsafe {
            ConnectNamedPipe(raw_handle, Some(ov))
        });
        if let Err(e) = connected
            && !matches!(os_error(&e), Some(ERROR_PIPE_CONNECTED | ERROR_NO_DATA))
        {
            // SAFETY: as above; resets an instance a client left half-open.
            let _ = unsafe { DisconnectNamedPipe(raw_handle) };
            return Err(e);
        }
        let mut client = 0u32;
        // SAFETY: client is a valid out pointer.
        let _ = unsafe { GetNamedPipeClientProcessId(raw_handle, &raw mut client) };
        Ok(Connection {
            instance: self,
            accepted: Instant::now(),
            client,
        })
    }
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

fn os_error(e: &io::Error) -> Option<WIN32_ERROR> {
    e.raw_os_error()
        .and_then(|c| u32::try_from(c).ok())
        .map(WIN32_ERROR)
}

/// An unnamed, manual-reset event for [`overlapped`].
fn event() -> io::Result<OwnedHandle> {
    // SAFETY: no attributes, no name.
    let h = unsafe { CreateEventW(None, true, false, None) }.map_err(win32)?;
    // SAFETY: h is a fresh, owned, valid handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(h.0) })
}

/// Turns a failed Win32 call back into the error code it carries, so callers
/// can match on it.
fn win32(e: windows::core::Error) -> io::Error {
    WIN32_ERROR::from_error(&e).map_or_else(
        || io::Error::other(e),
        |code| io::Error::from_raw_os_error(i32::try_from(code.0).unwrap_or(i32::MAX)),
    )
}

/// Starts one operation on a handle opened with `FILE_FLAG_OVERLAPPED` and
/// waits for it until `deadline`, or for as long as it takes without one. An
/// operation still pending at the deadline is cancelled and fails with
/// [`io::ErrorKind::TimedOut`]. Returns the bytes transferred.
///
/// The operation must not outlive this call, and it does not: a cancelled
/// one is waited for too before the `OVERLAPPED` goes out of scope.
fn overlapped(
    handle: HANDLE,
    event: HANDLE,
    deadline: Option<Instant>,
    start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
) -> io::Result<u32> {
    let mut ov = OVERLAPPED {
        hEvent: event,
        ..OVERLAPPED::default()
    };
    if let Err(e) = start(&raw mut ov) {
        let e = win32(e);
        if os_error(&e) != Some(ERROR_IO_PENDING) {
            return Err(e);
        }
        let ms = deadline.map_or(INFINITE, |d| {
            let left = d.saturating_duration_since(Instant::now()).as_millis();
            u32::try_from(left).unwrap_or(INFINITE - 1)
        });
        // SAFETY: event is a valid event handle.
        if unsafe { WaitForSingleObject(event, ms) } == WAIT_TIMEOUT {
            // SAFETY: ov is the pending operation's own OVERLAPPED.
            let _ = unsafe { CancelIoEx(handle, Some(&raw const ov)) };
        }
    }
    let mut done = 0u32;
    // SAFETY: ov and done are valid; waits until the operation has finished,
    // whether it completed or was cancelled.
    match unsafe { GetOverlappedResult(handle, &raw const ov, &raw mut done, true) }.map_err(win32)
    {
        Ok(()) => Ok(done),
        Err(e) if os_error(&e) == Some(ERROR_OPERATION_ABORTED) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the other end of the pipe did not answer in time",
        )),
        Err(e) => Err(e),
    }
}

/// Reads one line, at most [`MAX_LINE`] bytes, from an overlapped handle.
/// A peer that hangs up ends the line where it is.
fn read_line(handle: HANDLE, event: HANDLE, deadline: Option<Instant>) -> io::Result<String> {
    let mut line = Vec::new();
    let mut chunk = [0u8; 4096];
    while !line.contains(&b'\n') && line.len() < MAX_LINE {
        // SAFETY: chunk outlives the operation, which overlapped() waits for.
        let read = overlapped(handle, event, deadline, |ov| unsafe {
            ReadFile(handle, Some(&mut chunk), None, Some(ov))
        });
        match read {
            Ok(0) => break,
            Ok(n) => line.extend_from_slice(&chunk[..usize::try_from(n).unwrap_or(0)]),
            Err(e) if os_error(&e) == Some(ERROR_BROKEN_PIPE) => break,
            Err(e) => return Err(e),
        }
    }
    if let Some(end) = line.iter().position(|&b| b == b'\n') {
        line.truncate(end + 1);
    }
    line.truncate(MAX_LINE);
    String::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Writes all of `bytes` to an overlapped handle.
fn write_all(
    handle: HANDLE,
    event: HANDLE,
    mut bytes: &[u8],
    deadline: Option<Instant>,
) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: bytes outlives the operation, which overlapped() waits for.
        let written = overlapped(handle, event, deadline, |ov| unsafe {
            WriteFile(handle, Some(bytes), None, Some(ov))
        })?;
        if written == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        bytes = bytes
            .get(usize::try_from(written).unwrap_or(usize::MAX)..)
            .unwrap_or_default();
    }
    Ok(())
}

fn create(name: &str, sddl: &str, first: bool, max: u32) -> io::Result<OwnedHandle> {
    let sddl = to_wide(sddl);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: sddl is NUL terminated; sd receives a LocalAlloc'd descriptor
    // that is freed below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &raw mut sd,
            None,
        )
        .map_err(io::Error::other)?;
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    let name = to_wide(name);
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: name is NUL terminated and sa points at a live descriptor.
    let h = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            max,
            64 * 1024,
            64 * 1024,
            0,
            Some(&raw const sa),
        )
    };
    // SAFETY: sd came from the conversion above.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sd.0)));
    }
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: h is a fresh, owned, valid handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(h.0) })
}

impl Connection<'_> {
    /// Reads one line, at most 256 KiB, which must have arrived `within` the
    /// accept.
    ///
    /// # Errors
    ///
    /// When the read fails or times out. A client that hangs up first gives
    /// what it sent until then.
    pub fn read_line(&mut self, within: Duration) -> io::Result<String> {
        read_line(
            raw(&self.instance.handle),
            raw(&self.instance.event),
            Some(self.accepted + within),
        )
    }

    /// Writes one line and waits, at most `within`, for the client to read
    /// it and hang up, since disconnecting earlier would throw the line away.
    ///
    /// # Errors
    ///
    /// When the client is gone, or neither takes the line nor hangs up in
    /// time.
    pub fn write_line(&mut self, line: &str, within: Duration) -> io::Result<()> {
        let deadline = Some(Instant::now() + within);
        let (handle, event) = (raw(&self.instance.handle), raw(&self.instance.event));
        write_all(handle, event, line.as_bytes(), deadline)?;
        // The client reads its reply, then closes the pipe, which ends this
        // read. Anything else it sends meanwhile is ignored.
        let mut chunk = [0u8; 512];
        loop {
            // SAFETY: chunk outlives the operation, which overlapped() waits for.
            let read = overlapped(handle, event, deadline, |ov| unsafe {
                ReadFile(handle, Some(&mut chunk), None, Some(ov))
            });
            match read {
                Ok(_) => {}
                Err(e) if os_error(&e) == Some(ERROR_BROKEN_PIPE) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        // SAFETY: the instance's handle, which outlives the connection. A
        // reply was waited for in write_line, so nothing is left to flush.
        unsafe {
            let _ = DisconnectNamedPipe(raw(&self.instance.handle));
        }
    }
}

/// Sends one line and, for the control pipe, reads one back.
///
/// `wait` is how long to wait for a busy pipe, and then for the line to be
/// taken. The hook passes a short one: an agent must never be slowed down by
/// a daemon that is not answering, nor by a process posing as one.
///
/// # Errors
///
/// When no daemon is listening, the control server is not elevated, or the
/// exchange fails.
pub fn send(pipe: Pipe, line: &str, wait: Duration) -> io::Result<Option<String>> {
    send_to(&pipe.name(), pipe, line, wait)
}

fn send_to(name: &str, pipe: Pipe, line: &str, wait: Duration) -> io::Result<Option<String>> {
    let name = to_wide(name);
    // The hook asks for exactly what its access list grants the user.
    let access = match pipe {
        Pipe::Hook => FILE_WRITE_DATA.0,
        Pipe::Control => GENERIC_READ.0 | GENERIC_WRITE.0,
    };
    let open = || {
        // SAFETY: name is NUL terminated.
        unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                access,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                // Whoever answers may learn who the client is, never act as it.
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION | FILE_FLAG_OVERLAPPED,
                None,
            )
        }
    };
    let h = match open() {
        Ok(h) => h,
        Err(_) => {
            let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: name is NUL terminated.
            if !unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), ms) }.as_bool() {
                // The pipe is there but every instance stayed taken: the
                // daemon runs and is busy, for one with opening a fleet.
                // SAFETY: no arguments.
                if unsafe { GetLastError() } == ERROR_SEM_TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Daifuku is busy with another command; try again in a moment",
                    ));
                }
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Daifuku is not running; `daifuku doctor` says why",
                ));
            }
            open().map_err(io::Error::other)?
        }
    };
    // SAFETY: h is a fresh, owned, valid handle, closed when this returns.
    let _handle = unsafe { OwnedHandle::from_raw_handle(h.0) };
    if pipe == Pipe::Control {
        let mut server = 0u32;
        // SAFETY: server is a valid out pointer.
        let _ = unsafe { GetNamedPipeServerProcessId(h, &raw mut server) };
        if process::is_elevated(server) != Some(true) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the process answering on Daifuku's control pipe is not the elevated daemon",
            ));
        }
    }
    // A server that never reads, such as one that took the name while no
    // daemon ran, would otherwise hold the write for ever.
    let event = event()?;
    write_all(h, raw(&event), line.as_bytes(), Some(Instant::now() + wait))?;
    if pipe == Pipe::Hook {
        return Ok(None);
    }
    // The elevated daemon bounds how long it takes to answer.
    read_line(h, raw(&event), None).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY};

    /// A pipe name no daemon uses, different for every call.
    fn private_name() -> String {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(r"\\.\pipe\daifuku-test-{}-{n}", std::process::id())
    }

    fn os_error(e: &io::Error) -> Option<u32> {
        e.raw_os_error().and_then(|c| u32::try_from(c).ok())
    }

    #[test]
    fn names_are_per_session_and_distinct() {
        let (h, c) = (Pipe::Hook.name(), Pipe::Control.name());
        assert!(h.starts_with(r"\\.\pipe\daifuku-") && h.ends_with("-hook"));
        assert!(c.ends_with("-control"));
        assert_ne!(h, c);
    }

    #[test]
    fn the_control_pipe_admits_no_ordinary_user() {
        for elevated in [true, false] {
            let sddl = &Pipe::Control.sddl(elevated);
            assert!(!sddl.contains(";IU)") && !sddl.contains(";AU)") && !sddl.contains(";WD)"));
            assert!(sddl.contains("ML;;NW;;;HI"));
        }
    }

    #[test]
    fn the_hook_pipe_lets_the_user_write_through_a_medium_label() {
        let sddl = &Pipe::Hook.sddl(true);
        // FILE_GENERIC_READ and FILE_WRITE_DATA, not FILE_CREATE_PIPE_INSTANCE.
        let sid = crate::setup::user_sid().unwrap();
        assert!(sddl.contains(&format!("(A;;0x12008b;;;{sid})")));
        // Not every interactive user: only this one.
        assert!(!sddl.contains(";IU)"));
        assert!(sddl.contains("ML;;NW;;;ME"));
        // No execute, no delete, no ownership for the user.
        assert!(!sddl.contains(";GA;;;IU)"));
        assert!(&Pipe::Hook.sddl(false).contains("ML;;NW;;;ME"));
    }

    #[test]
    fn a_full_pipe_turns_away_another_server() {
        let name = private_name();
        let sddl = &Pipe::Hook.sddl(process::current_is_elevated());
        let _held = instances_at(&name, sddl, 2).unwrap();
        let Err(e) = create(&name, sddl, false, 2) else {
            panic!("a third server instance was created");
        };
        assert_eq!(os_error(&e), Some(ERROR_PIPE_BUSY.0));
    }

    #[test]
    fn an_ordinary_process_cannot_add_a_server_to_the_hook_pipe() {
        // Elevated, the test runs as an administrator, whom the list admits.
        if process::current_is_elevated() {
            return;
        }
        // With generic write the user may add an instance. If even that fails
        // here, this account is not interactive and the test proves nothing.
        let open = private_name();
        let _first = create(&open, &Pipe::Hook.sddl(false), true, 4).unwrap();
        if create(&open, &Pipe::Hook.sddl(false), false, 4).is_err() {
            return;
        }
        let name = private_name();
        let _first = create(&name, &Pipe::Hook.sddl(true), true, 4).unwrap();
        let Err(e) = create(&name, &Pipe::Hook.sddl(true), false, 4) else {
            panic!("an ordinary process added a server instance");
        };
        assert_eq!(os_error(&e), Some(ERROR_ACCESS_DENIED.0));
    }

    #[test]
    fn a_hook_line_gets_through_the_narrow_access_list() {
        let name = private_name();
        let mut instance = instances_at(&name, &Pipe::Hook.sddl(true), 1)
            .unwrap()
            .remove(0);
        let server = std::thread::spawn(move || {
            instance
                .accept()
                .unwrap()
                .read_line(Duration::from_millis(500))
                .unwrap()
        });
        send_to(&name, Pipe::Hook, "{\"hook\":1}\n", Duration::from_secs(2)).unwrap();
        assert_eq!(server.join().unwrap(), "{\"hook\":1}\n");
    }

    #[test]
    fn a_line_from_a_client_that_hung_up_before_the_accept_is_read() {
        let name = private_name();
        let sddl = &Pipe::Hook.sddl(process::current_is_elevated());
        let mut instance = instances_at(&name, sddl, 1).unwrap().remove(0);
        send_to(&name, Pipe::Hook, "{\"early\":1}\n", Duration::from_secs(2)).unwrap();
        let mut connection = instance.accept().unwrap();
        assert_eq!(
            connection.read_line(Duration::from_millis(500)).unwrap(),
            "{\"early\":1}\n"
        );
    }

    /// Opens `name` as a client that never writes anything.
    fn idle_client(name: &str) -> OwnedHandle {
        let name = to_wide(name);
        // SAFETY: name is NUL terminated.
        let h = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                FILE_WRITE_DATA.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        }
        .unwrap();
        // SAFETY: h is a fresh, owned, valid handle.
        unsafe { OwnedHandle::from_raw_handle(h.0) }
    }

    #[test]
    fn an_idle_client_does_not_hold_the_pipe() {
        let name = private_name();
        let sddl = &Pipe::Hook.sddl(process::current_is_elevated());
        let mut instance = instances_at(&name, sddl, 1).unwrap().remove(0);
        let (lines, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                if let Ok(mut connection) = instance.accept()
                    && let Ok(line) = connection.read_line(Duration::from_millis(500))
                    && lines.send(line).is_err()
                {
                    return;
                }
            }
        });
        let _idle = idle_client(&name);
        let sent = send_to(&name, Pipe::Hook, "{\"late\":1}\n", Duration::from_secs(3));
        assert!(sent.is_ok(), "the hook found no free instance: {sent:?}");
        let line = received.recv_timeout(Duration::from_secs(3));
        assert_eq!(line.as_deref(), Ok("{\"late\":1}\n"));
    }

    #[test]
    fn a_hook_does_not_hang_on_a_server_that_never_reads() {
        let name = private_name();
        let wide = to_wide(&name);
        // A squatter with no buffer, which never accepts and never reads.
        // SAFETY: wide is NUL terminated.
        let h = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                0,
                0,
                0,
                None,
            )
        };
        assert_ne!(h, INVALID_HANDLE_VALUE);
        // SAFETY: h is a fresh, owned, valid handle.
        let _squatter = unsafe { OwnedHandle::from_raw_handle(h.0) };
        let (done, finished) = std::sync::mpsc::channel();
        let line = format!("{}\n", "x".repeat(1024 * 1024));
        std::thread::spawn(move || {
            let _ = send_to(&name, Pipe::Hook, &line, Duration::from_millis(200));
            let _ = done.send(());
        });
        assert!(finished.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn a_client_that_never_reads_its_reply_is_let_go() {
        let name = private_name();
        let sddl = &Pipe::Hook.sddl(process::current_is_elevated());
        let mut instance = instances_at(&name, sddl, 1).unwrap().remove(0);
        let _idle = idle_client(&name);
        let start = Instant::now();
        let mut connection = instance.accept().unwrap();
        let written = connection.write_line("{}\n", Duration::from_millis(300));
        drop(connection);
        assert_eq!(written.map_err(|e| e.kind()), Err(io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn both_descriptors_parse() {
        for (pipe, elevated) in [
            (Pipe::Hook, true),
            (Pipe::Hook, false),
            (Pipe::Control, true),
        ] {
            let sddl = to_wide(&pipe.sddl(elevated));
            let mut sd = PSECURITY_DESCRIPTOR::default();
            // SAFETY: as in create().
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(sddl.as_ptr()),
                    SDDL_REVISION_1,
                    &raw mut sd,
                    None,
                )
                .unwrap();
                let _ = LocalFree(Some(HLOCAL(sd.0)));
            }
        }
    }

    #[test]
    fn sending_with_no_daemon_fails_fast() {
        // A session number no daemon uses: build the name by hand.
        let start = std::time::Instant::now();
        let r = send(Pipe::Hook, "{}\n", Duration::from_millis(50));
        // Either nothing listens (the normal case in a test run) or a real
        // daemon on this machine took the line; both are fine, slow is not.
        let _ = r;
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
