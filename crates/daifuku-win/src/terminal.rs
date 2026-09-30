//! Finding Windows Terminal and opening windows in it.
//!
//! The daemon runs elevated, so where it finds its programs matters: anything
//! it starts, starts as administrator. It therefore never searches `PATH`,
//! which has folders an ordinary process can write to. Windows Terminal is
//! looked up through its package, whose folder only the system can change,
//! and PowerShell through the Program Files and System32 known folders.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT,
    TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary,
};
use windows::Win32::Storage::Packaging::Appx::{
    FindPackagesByPackageFamily, GetPackagePathByFullName, PACKAGE_FILTER_HEAD,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessWithTokenW, LOGON_WITH_PROFILE, OpenProcess,
    OpenProcessToken, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{
    FOLDERID_ProgramFiles, FOLDERID_System, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
use windows::core::{PCWSTR, PWSTR};

use crate::wide::{from_wide, to_wide};

/// The package families Windows Terminal ships as, stable first.
const FAMILIES: [&str; 2] = [
    "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
    "Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe",
];

/// The window class every Windows Terminal window has.
pub const WINDOW_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

/// `wt.exe` inside the installed Windows Terminal package.
#[must_use]
pub fn find() -> Option<PathBuf> {
    FAMILIES
        .iter()
        .find_map(|family| package_path(family))
        .map(|p| p.join("wt.exe"))
        .filter(|p| p.is_file())
}

fn package_path(family: &str) -> Option<PathBuf> {
    let family_w = to_wide(family);
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: the first call only reports sizes; the second fills buffers of
    // exactly those sizes.
    unsafe {
        let _ = FindPackagesByPackageFamily(
            PCWSTR(family_w.as_ptr()),
            PACKAGE_FILTER_HEAD,
            &raw mut count,
            None,
            &raw mut len,
            None,
            None,
        );
        if count == 0 {
            return None;
        }
        let mut names = vec![PWSTR::null(); count as usize];
        let mut buffer = vec![0u16; len as usize];
        let status = FindPackagesByPackageFamily(
            PCWSTR(family_w.as_ptr()),
            PACKAGE_FILTER_HEAD,
            &raw mut count,
            Some(names.as_mut_ptr()),
            &raw mut len,
            Some(PWSTR(buffer.as_mut_ptr())),
            None,
        );
        if status.is_err() {
            return None;
        }
        let full_name = names.first().copied()?;
        let mut path_len = 0u32;
        let _ = GetPackagePathByFullName(PCWSTR(full_name.0), &raw mut path_len, None);
        let mut path = vec![0u16; path_len as usize];
        GetPackagePathByFullName(
            PCWSTR(full_name.0),
            &raw mut path_len,
            Some(PWSTR(path.as_mut_ptr())),
        )
        .ok()
        .ok()?;
        Some(PathBuf::from(from_wide(&path)))
    }
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    // SAFETY: the returned string is freed with CoTaskMemFree as documented.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = OsString::from_wide(p.as_wide());
        CoTaskMemFree(Some(p.0.cast()));
        Some(PathBuf::from(s))
    }
}

/// The PowerShell a fleet command runs in: PowerShell 7 if it is installed in
/// Program Files, else Windows PowerShell from System32.
#[must_use]
pub fn shell() -> Option<PathBuf> {
    let seven = known_folder(&FOLDERID_ProgramFiles).map(|p| p.join(r"PowerShell\7\pwsh.exe"));
    let five =
        known_folder(&FOLDERID_System).map(|p| p.join(r"WindowsPowerShell\v1.0\powershell.exe"));
    [seven, five].into_iter().flatten().find(|p| p.is_file())
}

/// What to open in one new Windows Terminal window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    /// Folder the tab starts in.
    pub directory: Option<PathBuf>,
    /// Windows Terminal profile name.
    pub profile: Option<String>,
    /// A command to run in PowerShell, which stays open after it exits.
    pub command: Option<String>,
    /// The tab title until the program inside sets its own.
    pub title: Option<String>,
    /// Skip the PowerShell profile: a faster start, and nothing from the
    /// user's own setup on screen.
    pub clean: bool,
}

impl Launch {
    /// The arguments for `wt.exe`, given the shell to run a command in.
    #[must_use]
    pub fn args(&self, shell: Option<&Path>) -> Vec<OsString> {
        let mut a: Vec<OsString> = ["-w", "new", "new-tab"]
            .iter()
            .map(OsString::from)
            .collect();
        if let Some(p) = &self.profile {
            a.push("--profile".into());
            a.push(escape(p).into());
        }
        if let Some(d) = &self.directory {
            a.push("--startingDirectory".into());
            a.push(escape(&d.to_string_lossy()).into());
        }
        if let Some(t) = &self.title {
            a.push("--title".into());
            a.push(escape(t).into());
        }
        if let (Some(cmd), Some(shell)) = (&self.command, shell) {
            a.push(shell.as_os_str().to_owned());
            if self.clean {
                a.push("-NoProfile".into());
            }
            for s in ["-NoLogo", "-NoExit", "-Command"] {
                a.push(s.into());
            }
            a.push(escape(cmd).into());
        }
        a
    }
}

/// Windows Terminal splits its command line on `;` into separate actions.
/// A literal one has to be written `\;`.
fn escape(s: &str) -> String {
    s.replace(';', r"\;")
}

/// Opens one window, elevated if this process is.
///
/// # Errors
///
/// When `wt.exe` cannot be started.
pub fn open(wt: &Path, launch: &Launch) -> std::io::Result<()> {
    // CREATE_NO_WINDOW: wt.exe itself is a console program that only hands
    // the request to the Windows Terminal process; without the flag it
    // flashes a console of its own.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new(wt)
        .args(launch.args(shell().as_deref()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(drop)
}

/// Opens one window **without** administrator rights, from an elevated
/// process, by starting it with the shell's own token: the desktop's
/// `explorer.exe` runs unelevated for the signed-in user.
///
/// # Errors
///
/// When there is no shell (a session without Explorer) or the start fails.
pub fn open_unelevated(wt: &Path, launch: &Launch) -> std::io::Result<()> {
    let mut line = quote(wt.as_os_str());
    for arg in launch.args(shell().as_deref()) {
        line.push(' ');
        line.push_str(&quote(&arg));
    }
    let mut line_w = to_wide(&line);
    let app = to_wide(&wt.to_string_lossy());
    // SAFETY: every handle opened here is closed before returning; the
    // strings are NUL terminated and outlive the call.
    unsafe {
        let shell_window = GetShellWindow();
        if shell_window.is_invalid() {
            return Err(std::io::Error::other(
                "no desktop shell to borrow a token from",
            ));
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell_window, Some(&raw mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(std::io::Error::other)?;
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_DUPLICATE, &raw mut token);
        let _ = CloseHandle(process);
        opened.map_err(std::io::Error::other)?;
        let mut primary = HANDLE::default();
        let access = TOKEN_ACCESS_MASK(
            TOKEN_QUERY.0
                | TOKEN_ASSIGN_PRIMARY.0
                | TOKEN_DUPLICATE.0
                | TOKEN_ADJUST_DEFAULT.0
                | TOKEN_ADJUST_SESSIONID.0,
        );
        let duplicated = DuplicateTokenEx(
            token,
            access,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &raw mut primary,
        );
        let _ = CloseHandle(token);
        duplicated.map_err(std::io::Error::other)?;
        let si = STARTUPINFOW {
            cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(0),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let started = CreateProcessWithTokenW(
            primary,
            LOGON_WITH_PROFILE,
            PCWSTR(app.as_ptr()),
            Some(PWSTR(line_w.as_mut_ptr())),
            CREATE_UNICODE_ENVIRONMENT,
            None,
            PCWSTR::null(),
            &raw const si,
            &raw mut pi,
        );
        let _ = CloseHandle(primary);
        started.map_err(std::io::Error::other)?;
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
    }
    Ok(())
}

/// Quotes one argument the way the Microsoft C runtime splits a command line:
/// backslashes are literal except before a quote, where they escape it.
fn quote(arg: &std::ffi::OsStr) -> String {
    let s = arg.to_string_lossy();
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        return s.into_owned();
    }
    let mut out = String::from('"');
    let mut backslashes = 0usize;
    for c in s.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_bare_launch_opens_a_new_window_with_the_default_profile() {
        assert_eq!(
            strs(&Launch::default().args(None)),
            ["-w", "new", "new-tab"]
        );
    }

    #[test]
    fn a_command_runs_in_powershell_that_stays_open() {
        let l = Launch {
            directory: Some(r"C:\src\Mochi".into()),
            command: Some("claude".into()),
            title: Some("agents 1".into()),
            profile: None,
            clean: false,
        };
        assert_eq!(
            strs(&l.args(Some(Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe")))),
            [
                "-w",
                "new",
                "new-tab",
                "--startingDirectory",
                r"C:\src\Mochi",
                "--title",
                "agents 1",
                r"C:\Program Files\PowerShell\7\pwsh.exe",
                "-NoLogo",
                "-NoExit",
                "-Command",
                "claude",
            ]
        );
    }

    #[test]
    fn a_command_without_a_shell_is_dropped_rather_than_run_bare() {
        let l = Launch {
            command: Some("claude".into()),
            ..Launch::default()
        };
        assert_eq!(strs(&l.args(None)), ["-w", "new", "new-tab"]);
    }

    #[test]
    fn semicolons_cannot_split_the_terminal_command_line() {
        let l = Launch {
            command: Some("cd x; claude".into()),
            ..Launch::default()
        };
        let args = strs(&l.args(Some(Path::new("pwsh.exe"))));
        assert_eq!(args.last().map(String::as_str), Some(r"cd x\; claude"));
    }

    #[test]
    fn quoting_follows_the_c_runtime_rules() {
        let q = |s: &str| quote(std::ffi::OsStr::new(s));
        assert_eq!(q("plain"), "plain");
        assert_eq!(q(""), "\"\"");
        assert_eq!(q(r"C:\Program Files\x.exe"), r#""C:\Program Files\x.exe""#);
        assert_eq!(q(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(q(r"ends in \"), r#""ends in \\""#);
        assert_eq!(q(r#"a\"b c"#), r#""a\\\"b c""#);
    }

    #[test]
    fn found_paths_live_where_only_the_system_writes() {
        if let Some(wt) = find() {
            assert!(
                wt.to_string_lossy().contains("WindowsApps"),
                "{}",
                wt.display()
            );
        }
        if let Some(sh) = shell() {
            let s = sh.to_string_lossy().to_ascii_lowercase();
            assert!(s.contains("program files") || s.contains("system32"), "{s}");
        }
    }
}
