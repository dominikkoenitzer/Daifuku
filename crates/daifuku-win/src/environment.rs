//! The machine `PATH`, and telling programs it changed.
//!
//! The value is read and written as stored. It is usually `REG_EXPAND_SZ`
//! with `%SystemRoot%` and other variables in it; expanding them on the way in
//! would write fixed folders back and break every entry that relies on one.
//! So it is read with `RegQueryValueExW`, which returns the raw value, never
//! with `RegGetValueW`, which expands it, and written back with its own type.

use std::io;
use std::path::Path;

use daifuku_core::path_list;
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SAM_FLAGS, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::PCWSTR;

use crate::wide::{from_wide, to_wide};

/// Where Windows keeps the machine environment.
const KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

/// The value's name. Registry names ignore case, this is how Windows writes it.
const NAME: &str = "Path";

/// The machine `PATH` as stored, with `%VAR%` left as written.
///
/// # Errors
///
/// When the key cannot be opened, the value is missing, or it is not a
/// string.
pub fn machine_path() -> io::Result<String> {
    let key = Key::open(KEY_QUERY_VALUE)?;
    read(&key).map(|(value, _)| value)
}

/// Adds `dir` to the end of the machine `PATH`, and returns whether it had to:
/// `false` when an entry already names it. Needs administrator rights.
///
/// # Errors
///
/// When the key cannot be opened for writing or the value cannot be read or
/// written.
pub fn add_to_machine_path(dir: &Path) -> io::Result<bool> {
    change(|value| path_list::add(value, &dir.to_string_lossy()))
}

/// Takes every entry that names `dir` out of the machine `PATH`, and returns
/// whether there was one. Needs administrator rights.
///
/// # Errors
///
/// When the key cannot be opened for writing or the value cannot be read or
/// written.
pub fn remove_from_machine_path(dir: &Path) -> io::Result<bool> {
    change(|value| path_list::remove(value, &dir.to_string_lossy()))
}

/// Tells every top-level window that the environment changed. Explorer then
/// reads it again, so terminals started from now on see the new `PATH`;
/// running ones keep the one they started with. A window that hangs is
/// skipped, and each gets at most five seconds.
pub fn broadcast_environment_change() {
    let area = to_wide("Environment");
    let mut result = 0usize;
    // SAFETY: area is NUL terminated and outlives the call, which returns only
    // once every window has answered or timed out; result is a valid out
    // pointer.
    let _ = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(area.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&raw mut result),
        )
    };
}

/// Reads the value, lets `edit` change it, and writes it back with the same
/// type when it did. A missing value counts as empty and is created as
/// `REG_EXPAND_SZ`, the type Windows itself uses.
fn change(edit: impl FnOnce(&str) -> Option<String>) -> io::Result<bool> {
    let key = Key::open(KEY_QUERY_VALUE | KEY_SET_VALUE)?;
    let (value, kind) = match read(&key) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => (String::new(), REG_EXPAND_SZ),
        r => r?,
    };
    let Some(changed) = edit(&value) else {
        return Ok(false);
    };
    write(&key, &changed, kind)?;
    Ok(true)
}

/// An open registry key, closed when dropped.
struct Key(HKEY);

impl Key {
    /// Opens the environment key with `access`.
    fn open(access: REG_SAM_FLAGS) -> io::Result<Self> {
        let path = to_wide(KEY);
        let mut key = HKEY::default();
        // SAFETY: path is NUL terminated and key is a valid out pointer.
        check(unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(path.as_ptr()),
                None,
                access,
                &raw mut key,
            )
        })?;
        Ok(Self(key))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened in Key::open and is closed only here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// The value and its type. Anything but a string is refused, so a value
/// this code does not understand is never written over.
fn read(key: &Key) -> io::Result<(String, REG_VALUE_TYPE)> {
    let name = to_wide(NAME);
    loop {
        let mut kind = REG_VALUE_TYPE::default();
        let mut len = 0u32;
        // SAFETY: name is NUL terminated; without a buffer the call only
        // reports the type and the size in bytes.
        check(unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&raw mut kind),
                None,
                Some(&raw mut len),
            )
        })?;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the machine Path is not a string",
            ));
        }
        // One more character than reported, since a stored string need not
        // end in a NUL.
        let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
        let mut size = u32::try_from(buf.len() * 2).unwrap_or(u32::MAX);
        // SAFETY: buf holds size bytes, and size tells the call so.
        let read = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&raw mut kind),
                Some(buf.as_mut_ptr().cast()),
                Some(&raw mut size),
            )
        };
        // Another program made the value longer in between: ask again.
        if read == ERROR_MORE_DATA {
            continue;
        }
        check(read)?;
        buf.truncate(size as usize / 2);
        return Ok((from_wide(&buf), kind));
    }
}

/// Writes `value` with type `kind`, NUL included as the registry expects.
fn write(key: &Key, value: &str, kind: REG_VALUE_TYPE) -> io::Result<()> {
    let name = to_wide(NAME);
    let data: Vec<u8> = to_wide(value)
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    // SAFETY: name is NUL terminated and data is a plain byte slice.
    check(unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, kind, Some(&data)) })
}

/// A registry result as an `io::Result`, keeping the Windows error code so
/// `NotFound` and `PermissionDenied` read as such.
pub(crate) fn check(e: WIN32_ERROR) -> io::Result<()> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(
            i32::try_from(e.0).unwrap_or(i32::MAX),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only reads. Nothing here writes the registry.
    #[test]
    fn reads_the_machine_path() {
        let path = machine_path().expect("the machine Path reads");
        assert!(!path.is_empty());
        assert!(!path.contains('\0'));
    }

    #[test]
    fn keeps_the_error_code() {
        let e = check(WIN32_ERROR(2)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.raw_os_error(), Some(2));
        assert!(check(ERROR_SUCCESS).is_ok());
    }
}
