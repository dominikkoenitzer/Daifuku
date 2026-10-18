//! Helpers the daemon's unit tests share.

use std::path::{Path, PathBuf};

use daifuku_win::process;
use windows::Win32::Storage::FileSystem::{
    DDD_NO_BROADCAST_SYSTEM, DDD_REMOVE_DEFINITION, DefineDosDeviceW,
};
use windows::core::PCWSTR;

/// A drive letter only the desktop user's own sign-in has, as `subst` in an
/// ordinary terminal makes one: an elevated process, with a sign-in of its
/// own, does not see it. Taken away again when dropped. Nothing is told
/// about it, so Explorer shows no new drive meanwhile.
pub struct UserDrive {
    device: Vec<u16>,
    target: Vec<u16>,
    root: PathBuf,
    /// One drive at a time: tests run side by side and would pick the same
    /// free letter.
    _one: std::sync::MutexGuard<'static, ()>,
}

/// Held by the drive there is.
static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl UserDrive {
    /// A drive for folder `dir`, or `None` where there is no such thing:
    /// this process is not elevated beside an ordinary desktop shell, as on
    /// CI, where both run with one sign-in, or every letter is taken, or the
    /// two sign-ins share their drives.
    pub fn new(dir: &Path) -> Option<Self> {
        if !process::current_is_elevated() || process::shell_is_elevated() != Some(false) {
            return None;
        }
        let one = ONE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seen =
            |root: &Path| root.exists() || process::as_shell_user(|| root.exists()).unwrap_or(true);
        let letter = ('P'..='Y')
            .rev()
            .find(|l| !seen(Path::new(&format!("{l}:\\"))))?;
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let drive = Self {
            device: wide(&format!("{letter}:")),
            target: wide(&dir.display().to_string()),
            root: PathBuf::from(format!("{letter}:\\")),
            _one: one,
        };
        // SAFETY: both strings are NUL terminated and outlive the call.
        let made = process::as_shell_user(|| unsafe {
            DefineDosDeviceW(
                DDD_NO_BROADCAST_SYSTEM,
                PCWSTR(drive.device.as_ptr()),
                PCWSTR(drive.target.as_ptr()),
            )
        });
        if !matches!(made, Ok(Ok(()))) {
            return None;
        }
        // Dropped from here on, so the drive goes whatever comes next.
        let user_sees = process::as_shell_user(|| drive.root.is_dir()).unwrap_or(false);
        (user_sees && !drive.root.is_dir()).then_some(drive)
    }

    /// The drive's root, `X:\`.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for UserDrive {
    fn drop(&mut self) {
        // SAFETY: both strings are NUL terminated and outlive the call.
        let _ = process::as_shell_user(|| unsafe {
            DefineDosDeviceW(
                DDD_REMOVE_DEFINITION | DDD_NO_BROADCAST_SYSTEM,
                PCWSTR(self.device.as_ptr()),
                PCWSTR(self.target.as_ptr()),
            )
        });
    }
}
