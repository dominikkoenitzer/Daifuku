//! Where Daifuku keeps its files.
//!
//! The config lives in `%ProgramData%\Daifuku`, not in the user profile, and
//! that is a security decision rather than a preference. The daemon may run
//! elevated and the config says what it starts and whether it is elevated, so
//! whoever could write the config could run anything as administrator. Every
//! ordinary process can write
//! the user profile, including by deleting a protected file and putting a new
//! one in its place, because the user owns the folder. `%ProgramData%\Daifuku`
//! is created by the installer with write access for administrators only.
//! So is the config in it while it asks for an elevated daemon. For an
//! ordinary daemon the installer also lets the user who installs write that
//! one file, which gives that user nothing their own daemon does not have.
//!
//! A daemon with the user's own rights cannot write there, and needs no
//! protection from the user: it keeps its state and logs under the user's
//! local app data instead. An elevated daemon never reads anything from that
//! folder.
//!
//! The paths come from the known folders API rather than the environment,
//! which the user can change. `ProgramData` is the exception: the known folder
//! API itself expands it from the environment, so it is found from `System32`.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Component, PathBuf};

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_ProgramFiles, FOLDERID_System,
    KF_FLAG_DEFAULT, SHGetKnownFolderPath,
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

/// `%ProgramData%\Daifuku`: the config, and the state and logs of an
/// elevated daemon.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    program_data().map(|p| p.join("Daifuku"))
}

/// `%LocalAppData%\Daifuku`: the state and logs of a daemon with the user's
/// own rights, which cannot write [`data_dir`].
#[must_use]
pub fn user_data_dir() -> Option<PathBuf> {
    known(&FOLDERID_LocalAppData).map(|p| p.join("Daifuku"))
}

/// Where a daemon keeps its state: [`data_dir`] when it is `elevated`, else
/// [`user_data_dir`].
#[must_use]
pub fn state_dir(elevated: bool) -> Option<PathBuf> {
    if elevated {
        data_dir()
    } else {
        user_data_dir()
    }
}

/// `ProgramData` at the root of the drive Windows is on.
///
/// Not the known folder: Windows stores that one as `%SystemDrive%\ProgramData`
/// and expands it with this process's environment, so a `SystemDrive` the user
/// set would move the config into a folder the user can write. `System32`
/// comes from the kernel, not the environment, and so does its drive.
fn program_data() -> Option<PathBuf> {
    let system = system_dir()?;
    let mut root = PathBuf::new();
    for c in system.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => root.push(c),
            _ => break,
        }
    }
    root.has_root().then(|| root.join("ProgramData"))
}

/// The config file.
#[must_use]
pub fn config_file() -> Option<PathBuf> {
    data_dir().map(|p| p.join("daifuku.json"))
}

/// Where the daemon writes its log: next to the config when it runs
/// elevated, else under the user's local app data.
#[must_use]
pub fn log_dir(elevated: bool) -> Option<PathBuf> {
    state_dir(elevated).map(|p| p.join("logs"))
}

/// `%ProgramFiles%\Daifuku`: where the installer puts the binaries.
#[must_use]
pub fn install_dir() -> Option<PathBuf> {
    known(&FOLDERID_ProgramFiles).map(|p| p.join("Daifuku"))
}

/// `System32`.
#[must_use]
pub fn system_dir() -> Option<PathBuf> {
    known(&FOLDERID_System)
}

/// The signed-in user's profile folder, `%USERPROFILE%`.
#[must_use]
pub fn profile_dir() -> Option<PathBuf> {
    known(&FOLDERID_Profile)
}

/// Claude Code's user settings, `%USERPROFILE%\.claude\settings.json`.
#[must_use]
pub fn claude_settings() -> Option<PathBuf> {
    known(&FOLDERID_Profile).map(|p| p.join(".claude").join("settings.json"))
}

/// Codex's hooks file, `%USERPROFILE%\.codex\hooks.json`.
#[must_use]
pub fn codex_hooks() -> Option<PathBuf> {
    known(&FOLDERID_Profile).map(|p| p.join(".codex").join("hooks.json"))
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
    fn program_data_is_on_the_windows_drive_where_the_known_folder_is() {
        let p = program_data().unwrap();
        let system = system_dir().unwrap();
        assert_eq!(p.parent(), system.ancestors().last(), "{}", p.display());
        let known = known(&windows::Win32::UI::Shell::FOLDERID_ProgramData).unwrap();
        assert!(
            p.as_os_str().eq_ignore_ascii_case(known.as_os_str()),
            "{}",
            p.display()
        );
    }

    #[test]
    fn only_an_elevated_daemon_keeps_its_state_and_logs_under_program_data() {
        assert_eq!(state_dir(true), data_dir());
        let mine = state_dir(false).unwrap();
        assert_eq!(Some(mine.clone()), user_data_dir());
        assert_ne!(Some(mine.clone()), data_dir());
        assert!(mine.ends_with("Daifuku"), "{}", mine.display());
        assert_eq!(log_dir(false), Some(mine.join("logs")));
        assert_eq!(log_dir(true), data_dir().map(|p| p.join("logs")));
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
