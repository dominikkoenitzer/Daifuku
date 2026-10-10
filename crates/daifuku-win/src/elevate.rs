//! Administrator rights and the console.
//!
//! Double-clicked in Explorer, the command line gets a console of its own and
//! no terminal to type a command in, so it offers to install instead. It asks
//! for administrator rights the way any installer does, through the prompt
//! Windows shows for the `runas` verb, and opens the config the same way.

use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_EXECUTABLE, AssocQueryStringW, SEE_MASK_NOASYNC,
    SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, PWSTR};

use crate::wide::{from_wide, to_wide};

/// Whether this process is alone in its console, as it is when Explorer
/// started it. In a terminal the shell shares the console, and with no
/// console at all there is nobody to ask.
#[must_use]
pub fn console_is_own() -> bool {
    let mut pids = [0u32; 2];
    // SAFETY: the buffer is a local array of the length passed.
    unsafe { GetConsoleProcessList(&mut pids) == 1 }
}

/// Lets go of this process's console. A console of its own, as Explorer
/// and Settings give a command line program, closes its window then, and
/// whatever is printed afterwards goes nowhere.
pub fn leave_console() {
    // SAFETY: FreeConsole only detaches this process from its console.
    let _ = unsafe { FreeConsole() };
}

/// Starts `program` with `parameters` as administrator, after the prompt
/// Windows shows for that. Returns once it has started, not when it ends.
///
/// # Errors
///
/// What Windows said; [`declined`] tells a declined prompt from a failure.
pub fn run_elevated(program: &Path, parameters: &str) -> io::Result<()> {
    execute("runas", program, parameters, false).map(drop)
}

/// [`run_elevated`], keeping hold of the process to wait for.
///
/// # Errors
///
/// What Windows said; [`declined`] tells a declined prompt from a failure.
pub fn start_elevated(program: &Path, parameters: &str) -> io::Result<Started> {
    execute("runas", program, parameters, true).map(Started)
}

/// A process [`start_elevated`] started.
pub struct Started(HANDLE);

impl Started {
    /// Waits for the process to end and returns its exit code.
    ///
    /// # Errors
    ///
    /// When Windows cannot say how it ended.
    pub fn wait(self) -> io::Result<u32> {
        // The shell may hand back no process, as for a document it passes on.
        if self.0.is_invalid() {
            return Ok(0);
        }
        let mut code = 0u32;
        // SAFETY: the handle is the process's, open until this is dropped.
        unsafe {
            WaitForSingleObject(self.0, INFINITE);
            GetExitCodeProcess(self.0, &raw mut code)?;
        }
        Ok(code)
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle came from ShellExecuteExW and is closed only here.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Starts `program` with `parameters` with this process's own rights.
///
/// # Errors
///
/// What Windows said.
pub fn run(program: &Path, parameters: &str) -> io::Result<()> {
    execute("open", program, parameters, false).map(drop)
}

/// Whether an error from [`run_elevated`] means the prompt was declined.
#[must_use]
pub fn declined(e: &io::Error) -> bool {
    e.raw_os_error() == i32::try_from(ERROR_CANCELLED.0).ok()
}

/// Starts `program` with `verb`. With `keep`, returns the process, which
/// the caller must close; without, no handle is asked for.
fn execute(verb: &str, program: &Path, parameters: &str, keep: bool) -> io::Result<HANDLE> {
    let verb = to_wide(verb);
    let program = to_wide(&program.to_string_lossy());
    let parameters = to_wide(parameters);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
        // The caller may exit right after this returns, as the offer to
        // install does: without this flag the start could still be on its
        // way.
        fMask: if keep {
            SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS
        } else {
            SEE_MASK_NOASYNC
        },
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(program.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: every string outlives the call, and a process handle is asked
    // for only with `keep`, which hands it to the caller to close.
    unsafe { ShellExecuteExW(&raw mut info) }
        .map(|()| info.hProcess)
        .map_err(|e| {
            // The shell answers in HRESULTs; a Win32 code inside one is kept as
            // the plain code, so it reads as text and compares as one.
            let code = e.code().0.cast_unsigned();
            if code & 0xFFFF_0000 == 0x8007_0000 {
                io::Error::from_raw_os_error(i32::try_from(code & 0xFFFF).unwrap_or(0))
            } else {
                io::Error::from(e)
            }
        })
}

/// The program Windows opens files ending in `extension` with, such as
/// `.json`. `None` when there is none, or when it cannot be started by its
/// path, as an app from the Store cannot.
#[must_use]
pub fn opens(extension: &str) -> Option<PathBuf> {
    let extension = to_wide(extension);
    let mut buf = [0u16; 1024];
    let mut len = u32::try_from(buf.len()).unwrap_or(0);
    // SAFETY: the buffer is local and its length in characters is passed.
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_EXECUTABLE,
            PCWSTR(extension.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR(buf.as_mut_ptr())),
            &raw mut len,
        )
    };
    if hr.is_err() {
        return None;
    }
    let path = PathBuf::from(from_wide(&buf));
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_runs_in_a_console_it_shares_or_none() {
        // Cargo and the shell that started it share the console, and CI has
        // none, so a test never looks started from Explorer.
        assert!(!console_is_own());
    }

    #[test]
    fn a_declined_prompt_is_told_from_a_failure() {
        assert!(declined(&io::Error::from_raw_os_error(1223)));
        assert!(!declined(&io::Error::from_raw_os_error(5)));
        assert!(!declined(&io::Error::other("no code")));
    }
}
