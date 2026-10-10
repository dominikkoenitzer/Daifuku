//! The entry Settings > Apps > Installed apps lists Daifuku by, and removes
//! it with. Under `HKEY_LOCAL_MACHINE`, as the install is for the machine.

use std::io;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
    REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW,
    RegSetValueExW,
};
use windows::core::PCWSTR;

use crate::environment::check;
use crate::wide::to_wide;

/// The entry's key under `HKEY_LOCAL_MACHINE`.
pub const KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Daifuku";

/// One value of the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A `REG_SZ` string.
    Text(String),
    /// A `REG_DWORD` number.
    Number(u32),
}

impl Value {
    /// The type and the bytes the registry stores it as: a string in UTF-16
    /// with its NUL, a number in four bytes, low byte first.
    fn data(&self) -> (REG_VALUE_TYPE, Vec<u8>) {
        match self {
            Self::Text(text) => (
                REG_SZ,
                to_wide(text)
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            ),
            Self::Number(n) => (REG_DWORD, n.to_le_bytes().to_vec()),
        }
    }
}

/// Writes the entry with `values`, making it or updating the one an earlier
/// install wrote. Needs administrator rights.
///
/// # Errors
///
/// When the key cannot be made or a value cannot be written.
pub fn write(values: &[(&str, Value)]) -> io::Result<()> {
    let path = to_wide(KEY);
    let mut key = HKEY::default();
    // SAFETY: path is NUL terminated and key is a valid out pointer.
    check(unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &raw mut key,
            None,
        )
    })?;
    let key = Key(key);
    for (name, value) in values {
        let name = to_wide(name);
        let (kind, data) = value.data();
        // SAFETY: name is NUL terminated and data is a plain byte slice.
        check(unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, kind, Some(&data)) })?;
    }
    Ok(())
}

/// Removes the entry, and returns whether there was one. Needs
/// administrator rights.
///
/// # Errors
///
/// When the key is there and cannot be removed.
pub fn remove() -> io::Result<bool> {
    let path = to_wide(KEY);
    // SAFETY: path is NUL terminated.
    match check(unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr())) }) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Whether the entry is there.
#[must_use]
pub fn exists() -> bool {
    let path = to_wide(KEY);
    let mut key = HKEY::default();
    // SAFETY: path is NUL terminated and key is a valid out pointer.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &raw mut key,
        )
    };
    if opened != ERROR_SUCCESS {
        return false;
    }
    drop(Key(key));
    true
}

/// An open registry key, closed when dropped.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened above and is closed only here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Nothing here writes the registry.
    #[test]
    fn a_string_is_stored_in_utf16_with_its_nul_and_a_number_in_four_bytes() {
        let (kind, data) = Value::Text("Dai".into()).data();
        assert_eq!(kind, REG_SZ);
        assert_eq!(data, [b'D', 0, b'a', 0, b'i', 0, 0, 0]);
        let (kind, data) = Value::Number(0x0001_0203).data();
        assert_eq!(kind, REG_DWORD);
        assert_eq!(data, [3, 2, 1, 0]);
    }
}
