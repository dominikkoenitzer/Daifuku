//! File times that hold the day and nothing finer. Windows writes the moment
//! of every change, and on most disks of every read, into a file's times; for
//! the files Daifuku keeps, the day is all those times may tell.

use std::fs::File;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, FILETIME, HANDLE, SYSTEMTIME};
use windows::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ, FILE_READ_ATTRIBUTES,
    FILE_WRITE_ATTRIBUTES, FileBasicInfo, GetFileInformationByHandleEx, SetFileInformationByHandle,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};

/// A time in [`FILE_BASIC_INFO`] that leaves the one on disk as it is.
const UNCHANGED: i64 = 0;

/// A time in [`FILE_BASIC_INFO`] that the file system stops moving for what
/// is done through the same handle from then on.
const HELD: i64 = -1;

/// The four times NTFS keeps for a file or folder, each in 100 ns ticks since
/// 1601 in UTC. Explorer shows the first three; `changed` moves with any
/// change, a rename or a new size included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Times {
    pub created: i64,
    pub accessed: i64,
    pub written: i64,
    pub changed: i64,
}

/// Local midnight at the start of today, as a file time.
///
/// # Errors
/// When the time zone cannot turn today's midnight into UTC.
pub fn start_of_today() -> io::Result<i64> {
    // SAFETY: no arguments; only reads the clock.
    let mut midnight = unsafe { GetLocalTime() };
    midnight.wHour = 0;
    midnight.wMinute = 0;
    midnight.wSecond = 0;
    midnight.wMilliseconds = 0;
    let mut utc = SYSTEMTIME::default();
    let mut time = FILETIME::default();
    // SAFETY: both point to live SYSTEMTIMEs and the FILETIME to one; None
    // takes the time zone Windows is set to, with its daylight saving rules
    // for that date.
    unsafe {
        TzSpecificLocalTimeToSystemTime(None, &raw const midnight, &raw mut utc)?;
        SystemTimeToFileTime(&raw const utc, &raw mut time)?;
    }
    Ok((i64::from(time.dwHighDateTime) << 32) | i64::from(time.dwLowDateTime))
}

/// The handle of `file`, for the calls that take one.
fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

/// Writes the four times of the file behind `handle`.
fn set(handle: HANDLE, created: i64, accessed: i64, written: i64, changed: i64) -> io::Result<()> {
    let info = FILE_BASIC_INFO {
        CreationTime: created,
        LastAccessTime: accessed,
        LastWriteTime: written,
        ChangeTime: changed,
        // 0 leaves the attributes as they are.
        FileAttributes: 0,
    };
    // SAFETY: the handle is open for the life of the call, and the buffer is
    // a FILE_BASIC_INFO of the size given.
    unsafe {
        SetFileInformationByHandle(
            handle,
            FileBasicInfo,
            (&raw const info).cast(),
            u32::try_from(size_of::<FILE_BASIC_INFO>()).unwrap_or(u32::MAX),
        )?;
    }
    Ok(())
}

/// Opens `path`, a file or a folder, with only the `access` asked for, and
/// shared with every other reader and writer.
fn open(path: &Path, access: u32) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .access_mode(access)
        .share_mode(7)
        // Needed to open a folder; changes nothing for a file.
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
}

/// The four times of the open `file`.
///
/// # Errors
/// When the handle has no right to read the attributes.
pub fn times_of(file: &File) -> io::Result<Times> {
    let mut info = FILE_BASIC_INFO::default();
    // SAFETY: the handle is open for the life of the call, and the buffer is
    // a FILE_BASIC_INFO of the size given.
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileBasicInfo,
            (&raw mut info).cast(),
            u32::try_from(size_of::<FILE_BASIC_INFO>()).unwrap_or(u32::MAX),
        )?;
    }
    Ok(Times {
        created: info.CreationTime,
        accessed: info.LastAccessTime,
        written: info.LastWriteTime,
        changed: info.ChangeTime,
    })
}

/// The four times of `path`, a file or a folder. Reading them reads no
/// data, so it moves none of them.
///
/// # Errors
/// When `path` does not open.
pub fn times(path: &Path) -> io::Result<Times> {
    times_of(&open(path, FILE_READ_ATTRIBUTES.0)?)
}

/// Sets every time of `path`, a file or a folder, to the start of today.
///
/// # Errors
/// When `path` does not open with the right to set its times.
pub fn keep_day_only(path: &Path) -> io::Result<()> {
    let day = start_of_today()?;
    let file = open(path, FILE_WRITE_ATTRIBUTES.0)?;
    set(handle(&file), day, day, day, day)
}

/// Sets every time of the open `file` to the start of today and holds them
/// there for whatever is written or read through it from now on, so a log
/// kept open keeps the day it was stamped, with no call per line. The handle
/// needs the right to write attributes, which a file opened to append has.
///
/// # Errors
/// When the handle has no right to set the times.
pub fn hold_day_only(file: &File) -> io::Result<()> {
    let day = start_of_today()?;
    set(handle(file), day, day, day, day)?;
    set(handle(file), UNCHANGED, HELD, HELD, HELD)
}

/// Reads `path` as text without leaving the moment of the read in its last
/// access time. A file the reader may not set times on, such as one another
/// user's daemon wrote, is read as it is.
///
/// # Errors
/// As [`std::fs::read_to_string`], the sharing violation of a file another
/// program holds included.
pub fn read_day_only(path: &Path) -> io::Result<String> {
    read_holding(path, |file| {
        set(handle(file), UNCHANGED, HELD, UNCHANGED, HELD)
    })
}

/// [`read_day_only`] with the step that holds the times passed in.
fn read_holding(path: &Path, hold: impl FnOnce(&File) -> io::Result<()>) -> io::Result<String> {
    let mut file = match open(path, FILE_GENERIC_READ.0 | FILE_WRITE_ATTRIBUTES.0) {
        Ok(file) => file,
        Err(e) if e.raw_os_error() == Some(i32::try_from(ERROR_ACCESS_DENIED.0).unwrap_or(5)) => {
            return std::fs::read_to_string(path);
        }
        Err(e) => return Err(e),
    };
    hold(&file)?;
    let mut text = String::new();
    io::Read::read_to_string(&mut file, &mut text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new empty folder for one test.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("daifuku-day-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn all(t: i64) -> Times {
        Times {
            created: t,
            accessed: t,
            written: t,
            changed: t,
        }
    }

    #[test]
    fn today_starts_at_local_midnight() {
        const TICKS_PER_DAY: i64 = 24 * 3600 * 10_000_000;
        let day = start_of_today().unwrap();
        // SAFETY: no arguments; only reads the clock.
        let now = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
        let now = (i64::from(now.dwHighDateTime) << 32) | i64::from(now.dwLowDateTime);
        // A day with a daylight saving change runs an hour long.
        assert!(
            day <= now && now - day < TICKS_PER_DAY + 3600 * 10_000_000,
            "{day} {now}"
        );
        let mut utc = SYSTEMTIME::default();
        let mut local = SYSTEMTIME::default();
        let time = FILETIME {
            dwLowDateTime: (day & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (day >> 32) as u32,
        };
        // SAFETY: each points to a live value of its type.
        unsafe {
            windows::Win32::System::Time::FileTimeToSystemTime(&raw const time, &raw mut utc)
                .unwrap();
            windows::Win32::System::Time::SystemTimeToTzSpecificLocalTime(
                None,
                &raw const utc,
                &raw mut local,
            )
            .unwrap();
        }
        // SAFETY: no arguments; only reads the clock.
        let today = unsafe { GetLocalTime() };
        assert_eq!(
            (
                local.wYear,
                local.wMonth,
                local.wDay,
                local.wHour,
                local.wMinute,
                local.wSecond
            ),
            (today.wYear, today.wMonth, today.wDay, 0, 0, 0)
        );
    }

    #[test]
    fn a_file_and_its_folder_keep_only_the_day() {
        let dir = scratch("keep");
        let file = dir.join("state.json");
        std::fs::write(&file, "{}").unwrap();
        keep_day_only(&file).unwrap();
        keep_day_only(&dir).unwrap();
        let day = start_of_today().unwrap();
        assert_eq!(times(&file).unwrap(), all(day));
        assert_eq!(times(&dir).unwrap(), all(day));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_held_file_keeps_the_day_while_it_is_written() {
        let dir = scratch("hold");
        let path = dir.join("daifukud.log");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        hold_day_only(&file).unwrap();
        for _ in 0..3 {
            io::Write::write_all(&mut file, b"a line that grows the file\n").unwrap();
            file.sync_all().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let day = start_of_today().unwrap();
        assert_eq!(times_of(&file).unwrap(), all(day));
        drop(file);
        assert_eq!(times(&path).unwrap(), all(day));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_leaves_the_access_time_on_the_day() {
        // Ten days back: NTFS moves an access time only when the read is an
        // hour or more past it, which a stamp of today's midnight is not
        // shortly after midnight.
        let old = start_of_today().unwrap() - 10 * 24 * 3600 * 10_000_000;
        let dir = scratch("read");
        let stamped = |name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, "[1]").unwrap();
            let file = open(&path, FILE_WRITE_ATTRIBUTES.0).unwrap();
            set(handle(&file), old, old, old, old).unwrap();
            path
        };
        let plain = stamped("plain.json");
        let guarded = stamped("peers.json");
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "[1]");
        assert_eq!(read_day_only(&guarded).unwrap(), "[1]");
        let moved = times(&plain).unwrap().accessed != old;
        assert_eq!(
            times(&guarded).unwrap(),
            all(old),
            "a plain read moved its file's: {moved}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_whose_times_cannot_be_held_is_read_all_the_same() {
        let dir = scratch("unheld");
        let path = dir.join("tasks.json");
        std::fs::write(&path, "[1]").unwrap();
        let text = read_holding(&path, |_| Err(io::Error::other("no times on this disk")));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(text.unwrap(), "[1]");
    }
}
