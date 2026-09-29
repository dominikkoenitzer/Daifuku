//! Where Daifuku keeps its files.
//!
//! The config lives in `%ProgramData%\Daifuku`, not in the user profile, and
//! that is a security decision rather than a preference. The daemon runs
//! elevated and the config says what it starts, so whoever can write the
//! config can run anything as administrator. Every ordinary process can write
//! the user profile, including by deleting a protected file and putting a new
//! one in its place, because the user owns the folder. `%ProgramData%\Daifuku`
//! is created by the installer with write access for administrators only.
//!
//! The paths come from the known folders API rather than the environment,
//! which the user can change.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_ProgramFiles, KF_FLAG_DEFAULT,
    SHGetKnownFolderPath,
};

fn known(id: &windows::core::GUID) -> Option<PathBuf> {
    // SAFETY: the returned string is freed with CoTaskMemFree as documented.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = OsString::from_wide(p.as_wide());
        CoTaskMemFree(Some(p.0.cast()));
        Some(PathBuf::from(s))
    }
}

/// `%ProgramData%\Daifuku`: the config and the daemon's logs.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    known(&FOLDERID_ProgramData).map(|p| p.join("Daifuku"))
}

/// The config file.
#[must_use]
pub fn config_file() -> Option<PathBuf> {
    data_dir().map(|p| p.join("daifuku.json"))
}

/// Where the daemon writes its log: next to the config when that folder is
/// writable (an installed, elevated daemon), else under the user's local app
/// data (a development run).
#[must_use]
pub fn log_dir(elevated: bool) -> Option<PathBuf> {
    if elevated {
        data_dir().map(|p| p.join("logs"))
    } else {
        known(&FOLDERID_LocalAppData).map(|p| p.join("Daifuku").join("logs"))
    }
}

/// `%ProgramFiles%\Daifuku`: where the installer puts the binaries.
#[must_use]
pub fn install_dir() -> Option<PathBuf> {
    known(&FOLDERID_ProgramFiles).map(|p| p.join("Daifuku"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_sits_under_program_data() {
        let c = config_file().unwrap();
        assert!(c.ends_with(r"Daifuku\daifuku.json"), "{}", c.display());
        assert!(
            !c.to_string_lossy()
                .to_ascii_lowercase()
                .contains(r"\users\"),
            "{}",
            c.display()
        );
    }

    #[test]
    fn the_install_dir_sits_under_program_files() {
        let d = install_dir().unwrap();
        assert!(
            d.to_string_lossy()
                .to_ascii_lowercase()
                .contains("program files"),
            "{}",
            d.display()
        );
    }
}
