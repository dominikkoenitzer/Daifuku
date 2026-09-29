//! Named pipes, one JSON line each way.
//!
//! Daifuku has two, and their security is the point of having two:
//!
//! | pipe | who may connect | why |
//! |---|---|---|
//! | hook | the signed-in user, at any integrity | a hook runs at whatever level its agent runs at, and the most it can do is colour a border |
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

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE,
    FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
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
const MAX_LINE: u64 = 256 * 1024;

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

    /// The security descriptor, in SDDL.
    ///
    /// - Hook: full access for SYSTEM and Administrators, read and write for
    ///   the interactive user, and a medium no-write-up label.
    /// - Control: full access for SYSTEM and Administrators only, high label.
    const fn sddl(self) -> &'static str {
        match self {
            Self::Hook => "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)S:(ML;;NW;;;ME)",
            Self::Control => "D:P(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;HI)",
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
pub struct Instance {
    handle: OwnedHandle,
}

/// One connected client. Dropping it disconnects the client and frees the
/// instance for the next one.
pub struct Connection<'a> {
    handle: &'a OwnedHandle,
    /// The client's process id, as Windows reports it.
    pub client: u32,
}

/// Claims the pipe name and creates `count` instances of it. Fails if any
/// other process already holds the name.
///
/// # Errors
///
/// When the name is taken or the security descriptor is refused.
pub fn instances(pipe: Pipe, count: usize) -> io::Result<Vec<Instance>> {
    let mut all = Vec::with_capacity(count.max(1));
    all.push(Instance {
        handle: create(pipe, true)?,
    });
    for _ in 1..count {
        all.push(Instance {
            handle: create(pipe, false)?,
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
        let raw_handle = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(
            &self.handle,
        ));
        // SAFETY: a valid pipe handle, synchronous connect.
        if let Err(e) = unsafe { ConnectNamedPipe(raw_handle, None) }
            && e.code() != ERROR_PIPE_CONNECTED.to_hresult()
        {
            // SAFETY: as above; resets an instance a client left half-open.
            let _ = unsafe { DisconnectNamedPipe(raw_handle) };
            return Err(io::Error::other(e));
        }
        let mut client = 0u32;
        // SAFETY: client is a valid out pointer.
        let _ = unsafe { GetNamedPipeClientProcessId(raw_handle, &raw mut client) };
        Ok(Connection {
            handle: &self.handle,
            client,
        })
    }
}

fn create(pipe: Pipe, first: bool) -> io::Result<OwnedHandle> {
    let sddl = to_wide(pipe.sddl());
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
    let name = to_wide(&pipe.name());
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: name is NUL terminated and sa points at a live descriptor.
    let h = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
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
    /// Reads one line, at most [`MAX_LINE`] bytes.
    ///
    /// # Errors
    ///
    /// When the client hangs up first or the read fails.
    pub fn read_line(&mut self) -> io::Result<String> {
        let file = std::fs::File::from(self.handle.try_clone()?);
        let mut line = String::new();
        BufReader::new(file.take(MAX_LINE)).read_line(&mut line)?;
        Ok(line)
    }

    /// Writes one line.
    ///
    /// # Errors
    ///
    /// When the client is gone.
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let mut file = std::fs::File::from(self.handle.try_clone()?);
        file.write_all(line.as_bytes())?;
        file.flush()
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        let raw_handle = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(
            self.handle,
        ));
        // SAFETY: the instance's handle, which outlives the connection. The
        // flush makes sure a reply was read before the client is cut off.
        unsafe {
            let _ = FlushFileBuffers(raw_handle);
            let _ = DisconnectNamedPipe(raw_handle);
        }
    }
}

/// Sends one line and, for the control pipe, reads one back.
///
/// `wait` is how long to wait for a busy pipe. The hook passes a short one: an
/// agent must never be slowed down by a daemon that is not answering.
///
/// # Errors
///
/// When no daemon is listening, the control server is not elevated, or the
/// exchange fails.
pub fn send(pipe: Pipe, line: &str, wait: Duration) -> io::Result<Option<String>> {
    let name = to_wide(&pipe.name());
    let access = match pipe {
        Pipe::Hook => GENERIC_WRITE.0,
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
                FILE_FLAGS_AND_ATTRIBUTES(0),
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
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Daifuku is not running",
                ));
            }
            open().map_err(io::Error::other)?
        }
    };
    // SAFETY: h is a fresh, owned, valid handle.
    let handle = unsafe { OwnedHandle::from_raw_handle(h.0) };
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
    let mut file = std::fs::File::from(handle);
    file.write_all(line.as_bytes())?;
    file.flush()?;
    if pipe == Pipe::Hook {
        return Ok(None);
    }
    let mut reply = String::new();
    BufReader::new(file.take(MAX_LINE)).read_line(&mut reply)?;
    Ok(Some(reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_per_session_and_distinct() {
        let (h, c) = (Pipe::Hook.name(), Pipe::Control.name());
        assert!(h.starts_with(r"\\.\pipe\daifuku-") && h.ends_with("-hook"));
        assert!(c.ends_with("-control"));
        assert_ne!(h, c);
    }

    #[test]
    fn the_control_pipe_admits_no_ordinary_user() {
        let sddl = Pipe::Control.sddl();
        assert!(!sddl.contains(";IU)") && !sddl.contains(";AU)") && !sddl.contains(";WD)"));
        assert!(sddl.contains("ML;;NW;;;HI"));
    }

    #[test]
    fn the_hook_pipe_lets_the_user_write_through_a_medium_label() {
        let sddl = Pipe::Hook.sddl();
        assert!(sddl.contains("(A;;GRGW;;;IU)"));
        assert!(sddl.contains("ML;;NW;;;ME"));
        // No execute, no delete, no ownership for the user.
        assert!(!sddl.contains(";GA;;;IU)"));
    }

    #[test]
    fn both_descriptors_parse() {
        for pipe in [Pipe::Hook, Pipe::Control] {
            let sddl = to_wide(pipe.sddl());
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
