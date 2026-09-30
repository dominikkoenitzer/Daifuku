//! What installing needs from Windows: the user's SID, a folder only
//! administrators can change, and Task Scheduler.

use std::path::Path;
use std::process::Command;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
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

/// Creates `dir` if needed and makes it a folder only administrators can
/// change, including everything already in it.
///
/// Ownership matters as much as the access list: the owner of a file may
/// always rewrite its access list. A folder or file that an ordinary process
/// created before the install would keep that process's owner, and with it a
/// way back in. So the folder and every entry under it are given to the
/// Administrators group, and the entries' own access lists are replaced by
/// what they inherit from the folder.
///
/// # Errors
///
/// When the folder cannot be created or a security call is refused.
pub fn harden_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let sddl = to_wide(FOLDER_SDDL);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: sddl is NUL terminated; sd is freed at the end. The owner and
    // DACL pointers point into sd and are only used while it lives.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &raw mut sd,
            None,
        )
        .map_err(std::io::Error::other)?;
        let mut owner = PSID::default();
        let mut defaulted = false.into();
        GetSecurityDescriptorOwner(sd, &raw mut owner, &raw mut defaulted)
            .map_err(std::io::Error::other)?;
        let mut present = false.into();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        GetSecurityDescriptorDacl(sd, &raw mut present, &raw mut dacl, &raw mut defaulted)
            .map_err(std::io::Error::other)?;

        let result = (|| {
            set(
                dir,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                owner,
                dacl,
            )?;
            // Every entry below: owner Administrators, no access list of its
            // own, only what it inherits from the folder.
            let empty = empty_acl();
            for entry in walk(dir) {
                set(
                    &entry,
                    OWNER_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION
                        | UNPROTECTED_DACL_SECURITY_INFORMATION,
                    owner,
                    empty.as_ptr().cast_mut().cast(),
                )?;
            }
            Ok(())
        })();
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        result
    }
}

/// An ACL with no entries, as a buffer: a header of revision 2 and nothing
/// else. Passing it with `UNPROTECTED_DACL_SECURITY_INFORMATION` leaves an
/// object with exactly the entries it inherits.
fn empty_acl() -> Vec<u64> {
    // ACL header: AclRevision u8, Sbz1 u8, AclSize u16, AceCount u16, Sbz2 u16.
    // Eight bytes, kept in a u64 so the buffer is aligned.
    let size: u16 = 8;
    let header = u64::from(2u8) | (u64::from(size) << 16);
    vec![header]
}

fn set(
    path: &Path,
    what: windows::Win32::Security::OBJECT_SECURITY_INFORMATION,
    owner: PSID,
    dacl: *mut ACL,
) -> std::io::Result<()> {
    let p = to_wide(&path.to_string_lossy());
    // SAFETY: p is NUL terminated; owner and dacl are valid for the call.
    let r = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(p.as_ptr()),
            SE_FILE_OBJECT,
            what,
            Some(owner),
            None,
            Some(dacl),
            None,
        )
    };
    if r.is_err() {
        return Err(std::io::Error::from_raw_os_error(
            i32::try_from(r.0).unwrap_or(-1),
        ));
    }
    Ok(())
}

/// Every file and folder under `dir`, not following links out of it.
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let is_real_dir = e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink());
            out.push(path.clone());
            if is_real_dir {
                stack.push(path);
            }
        }
    }
    out
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
    fn the_empty_acl_is_a_bare_header() {
        let acl = empty_acl();
        // SAFETY: reading the header bytes of our own buffer.
        let bytes: [u8; 8] = acl[0].to_le_bytes();
        assert_eq!(bytes[0], 2, "revision");
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 8, "size");
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 0, "no entries");
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
