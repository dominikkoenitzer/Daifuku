//! What installing needs from Windows: the user's SID, a folder only
//! administrators can change, Task Scheduler, and a delete at the next
//! restart for what uninstalling could not remove.

use std::path::Path;
use std::process::Command;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
    IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::Storage::FileSystem::{
    CreateDirectoryW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW, READ_CONTROL,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

use crate::wide::{from_wide, to_wide};

/// The signed-in user's SID as a string, `S-1-5-21-...`.
#[must_use]
pub fn user_sid() -> Option<String> {
    // SAFETY: the token is closed and the SID string freed before returning;
    // the buffer is sized by the first call.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &raw mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &raw mut len,
        );
        let _ = CloseHandle(token);
        ok.ok()?;
        let user = &*(buf.as_ptr().cast::<TOKEN_USER>());
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &raw mut s).ok()?;
        let out = from_wide(s.as_wide());
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        Some(out)
    }
}

/// The folder's access list: SYSTEM and Administrators may do anything, every
/// other user may read. Protected, so nothing is inherited from
/// `%ProgramData%`, which lets every user create files.
const FOLDER_SDDL: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";

/// Makes `dir` a folder only administrators can change, and returns whether
/// what was already there had to be removed.
///
/// `%ProgramData%` lets every user create folders, so an ordinary process may
/// have made `dir` before the install, filled it with a config of its own, or
/// made it a link to somewhere else. Taking such a folder over would keep what
/// it holds, and changing the security of a link changes whatever it points
/// to. So a folder is kept, with everything in it, only when it is a real
/// folder that an administrator owns and no one else may write: one this
/// installer made before. Anything else at `dir` is deleted without following
/// links in it, and a new folder is created with the access list already in
/// place, so it is never open to anyone.
///
/// # Errors
///
/// When the old folder cannot be removed, the new one cannot be created
/// (also when something appeared at `dir` in between), or a security call is
/// refused.
pub fn harden_dir(dir: &Path) -> std::io::Result<bool> {
    let removed = match trusted(dir) {
        Ok(true) => return Ok(false),
        Ok(false) => {
            // Does not follow links: a link is removed, not its target.
            match std::fs::remove_dir_all(dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => {
                    std::fs::remove_file(dir)?;
                }
                r => r?,
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let sddl = to_wide(FOLDER_SDDL);
    let path = to_wide(&dir.to_string_lossy());
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: both strings are NUL terminated; sd lives until it is freed
    // after the folder is created.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &raw mut sd,
            None,
        )
        .map_err(std::io::Error::other)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        // Fails when anything exists at the path, so a folder someone made
        // after the removal above is never taken over.
        let r = CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&raw const attributes));
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        r.map_err(std::io::Error::from)?;
    }
    Ok(removed)
}

/// Whether `dir` is a real folder, not a link, owned by Administrators or
/// SYSTEM, with an access list that lets no one else write, delete or change
/// it. The folder is opened without following a link, so its answer is about
/// the folder at `dir` itself.
///
/// # Errors
///
/// When `dir` cannot be opened, `NotFound` when there is nothing there.
fn trusted(dir: &Path) -> std::io::Result<bool> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;

    let file = std::fs::OpenOptions::new()
        .access_mode(READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(dir)?;
    let attributes = file.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
    {
        return Ok(false);
    }
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the handle is open for the call; owner and dacl point into sd,
    // which is freed after the last use of either.
    unsafe {
        let r = GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&raw mut owner),
            None,
            Some(&raw mut dacl),
            None,
            Some(&raw mut sd),
        );
        if r.is_err() {
            return Err(std::io::Error::from_raw_os_error(
                i32::try_from(r.0).unwrap_or(-1),
            ));
        }
        let ok = is_admin(owner) && !dacl.is_null() && only_admins_write(dacl);
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        Ok(ok)
    }
}

/// Whether the SID is Administrators or SYSTEM.
///
/// # Safety
///
/// `sid` must point to a valid SID.
unsafe fn is_admin(sid: PSID) -> bool {
    // SAFETY: as the caller promises.
    unsafe {
        IsWellKnownSid(sid, WinBuiltinAdministratorsSid).as_bool()
            || IsWellKnownSid(sid, WinLocalSystemSid).as_bool()
    }
}

/// Whether every entry of the access list that grants a right to change the
/// folder or what is in it grants it to Administrators or SYSTEM. An entry of
/// a kind this does not read counts as granting everything.
///
/// # Safety
///
/// `dacl` must point to a valid ACL.
unsafe fn only_admins_write(dacl: *mut ACL) -> bool {
    /// Write data, append, write extended attributes, delete a child, write
    /// attributes, delete, change the access list, take ownership, and the
    /// generic write and all.
    const WRITE: u32 = 0x2
        | 0x4
        | 0x10
        | 0x40
        | 0x100
        | 0x1_0000
        | 0x4_0000
        | 0x8_0000
        | 0x1000_0000
        | 0x4000_0000;
    const ALLOWED: u8 = 0;
    const DENIED: u8 = 1;
    // SAFETY: as the caller promises; GetAce only hands out entries inside
    // the list, each starting with its header.
    unsafe {
        for i in 0..u32::from((*dacl).AceCount) {
            let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
            if GetAce(dacl, i, &raw mut ace).is_err() {
                return false;
            }
            let header = &*ace.cast::<ACE_HEADER>();
            match header.AceType {
                DENIED => {}
                ALLOWED => {
                    let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    let sid = PSID((&raw const allowed.SidStart).cast_mut().cast());
                    if allowed.Mask & WRITE != 0 && !is_admin(sid) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
    true
}

/// Has Windows delete `path`, a file or an empty folder, at the next restart,
/// before anything can run from it. Only administrators may.
///
/// # Errors
///
/// When Windows refuses.
pub fn delete_at_restart(path: &Path) -> std::io::Result<()> {
    let path = to_wide(&path.to_string_lossy());
    // SAFETY: the path is NUL terminated; no new name means delete.
    unsafe {
        MoveFileExW(
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    }
    .map_err(std::io::Error::from)
}

/// Registers the task from its XML, replacing any older version.
///
/// `schtasks` reads the XML from a file, and that file is written into `dir`,
/// which must be a folder only administrators can change: in the temp folder
/// an ordinary process could swap it for a task that runs its own program.
///
/// # Errors
///
/// When `schtasks` fails; its own message is in the error.
pub fn create_task(name: &str, xml: &str, dir: &Path) -> std::io::Result<()> {
    let file = dir.join("task.xml");
    // schtasks reads the XML as UTF-16 with a byte order mark.
    let mut bytes = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&file, bytes)?;
    let r = schtasks(&[
        "/Create",
        "/TN",
        name,
        "/XML",
        &file.to_string_lossy(),
        "/F",
    ]);
    let _ = std::fs::remove_file(&file);
    r
}

/// Starts the task now.
///
/// # Errors
///
/// When `schtasks` fails.
pub fn run_task(name: &str) -> std::io::Result<()> {
    schtasks(&["/Run", "/TN", name])
}

/// Deletes the task. A task that does not exist is not an error.
///
/// # Errors
///
/// When `schtasks` fails for another reason.
pub fn delete_task(name: &str) -> std::io::Result<()> {
    if !task_exists(name) {
        return Ok(());
    }
    schtasks(&["/Delete", "/TN", name, "/F"])
}

/// Whether the task is registered.
#[must_use]
pub fn task_exists(name: &str) -> bool {
    schtasks(&["/Query", "/TN", name]).is_ok()
}

fn schtasks(args: &[&str]) -> std::io::Result<()> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    use std::os::windows::process::CommandExt;
    let exe = system32().join("schtasks.exe");
    let out = Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(std::io::Error::other(if msg.is_empty() {
            format!("schtasks {} failed", args[0])
        } else {
            msg
        }))
    }
}

/// `System32`, from the known folder rather than `%SystemRoot%`.
fn system32() -> std::path::PathBuf {
    crate::paths::system_dir().unwrap_or_else(|| r"C:\Windows\System32".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_user_has_a_sid() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
    }

    #[test]
    fn a_folder_this_user_made_is_not_trusted_and_nothing_is_not_there() {
        let dir = std::env::temp_dir().join(format!("daifuku-trust-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        assert!(!trusted(&dir).unwrap());
        std::fs::remove_dir(&dir).unwrap();
        assert_eq!(
            trusted(&dir).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn a_link_is_judged_as_a_link_not_as_its_target() {
        // A junction, which any user may make, to a folder with a config in
        // it.
        let base = std::env::temp_dir().join(format!("daifuku-link-{}", std::process::id()));
        let target = base.join("target");
        let link = base.join("link");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("daifuku.json"), "{}").unwrap();
        let made = Command::new(system32().join("cmd.exe"))
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(!trusted(&link).unwrap());
        // What harden_dir removes: the junction goes, its target stays.
        std::fs::remove_dir_all(&link).unwrap();
        assert!(!link.exists());
        assert!(target.join("daifuku.json").is_file());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_folder_descriptor_parses_and_admits_no_ordinary_writer() {
        assert!(FOLDER_SDDL.contains("D:P"), "protected");
        assert!(FOLDER_SDDL.starts_with("O:BA"), "owned by Administrators");
        assert!(!FOLDER_SDDL.contains("FA;;;BU") && !FOLDER_SDDL.contains(";;;WD)"));
        let w = to_wide(FOLDER_SDDL);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: as in harden_dir.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(w.as_ptr()),
                SDDL_REVISION_1,
                &raw mut sd,
                None,
            )
            .unwrap();
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }
    }
}
