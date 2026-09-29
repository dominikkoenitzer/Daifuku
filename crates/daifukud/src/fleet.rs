//! Opening fleets and keeping their terminals in their cells.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use daifuku_core::Rect;
use daifuku_core::config::{Config, Fleet};
use daifuku_core::grid::Grid;
use daifuku_core::monitor::{MonitorInfo, pick};
use daifuku_win::terminal::{self, Launch, WINDOW_CLASS};
use daifuku_win::{monitor, window};

/// How long Windows Terminal gets to show a window. A cold start of the first
/// window on a busy machine is the slow case; later ones take a few hundred
/// milliseconds.
const SHOW_TIMEOUT: Duration = Duration::from_secs(12);

/// A fleet that is open.
#[derive(Debug, Clone)]
pub struct OpenFleet {
    /// The config's name for it.
    pub name: String,
    /// The monitor it opened on, as picked then.
    pub monitor: String,
    /// Its terminals, in cell order.
    pub windows: Vec<u64>,
}

impl OpenFleet {
    /// Drops windows that no longer exist or stopped being terminals (a
    /// handle reused by some other window).
    pub fn prune(&mut self) {
        self.windows
            .retain(|&w| window::exists(w) && window::class(w) == WINDOW_CLASS);
    }
}

/// Where a fleet's cells are right now, with the monitor recomputed so a fleet
/// follows a monitor that was unplugged and plugged back.
fn cells(fleet: &Fleet, config: &Config) -> anyhow::Result<(MonitorInfo, Vec<Rect>)> {
    let monitors = monitor::monitors();
    let m = pick(&monitors, &fleet.monitor, monitor::cursor())
        .cloned()
        .ok_or_else(|| anyhow!("no monitor"))?;
    let cells = Grid::plan(fleet.count, m.work, config.gaps, fleet.shape);
    Ok((m, cells))
}

/// Every Windows Terminal window on the desktop right now.
fn terminals() -> BTreeSet<u64> {
    window::top_level()
        .into_iter()
        .filter(|&w| window::class(w) == WINDOW_CLASS)
        .collect()
}

/// Opens `fleet`, or brings back the one that is open: missing terminals are
/// opened again, and all of them go back to their cells.
///
/// `open` is the fleet's current record, if it has one. Returns the new
/// record and a line for the person who asked.
pub fn open(
    fleet: &Fleet,
    config: &Config,
    open: Option<OpenFleet>,
    elevated: bool,
) -> anyhow::Result<(OpenFleet, String)> {
    let (m, cells) = cells(fleet, config)?;
    let mut record = open.unwrap_or_else(|| OpenFleet {
        name: fleet.name.clone(),
        monitor: m.device.clone(),
        windows: Vec::new(),
    });
    record.prune();
    record.monitor = m.device.clone();
    let missing = cells.len().saturating_sub(record.windows.len());

    if missing > 0 {
        if fleet.admin && !elevated {
            return Err(anyhow!(
                "fleet `{}` opens administrator terminals, and this daemon is not elevated. Install Daifuku, or set \"admin\": false",
                fleet.name
            ));
        }
        let wt = terminal::find().context("Windows Terminal is not installed")?;
        let before = terminals();
        let started = record.windows.len();
        for i in 0..missing {
            let launch = Launch {
                directory: Some(directory(fleet, started + i + 1)),
                profile: fleet.profile.clone(),
                command: fleet
                    .command
                    .as_ref()
                    .map(|c| c.replace("{n}", &(started + i + 1).to_string())),
                title: Some(format!("{} {}", fleet.name, started + i + 1)),
                clean: fleet.no_profile,
            };
            let r = if fleet.admin || !elevated {
                terminal::open(&wt, &launch)
            } else {
                terminal::open_unelevated(&wt, &launch)
            };
            r.with_context(|| format!("could not start Windows Terminal at {}", wt.display()))?;
        }
        collect(&mut record, &before, missing, &cells);
    }

    place_all(&record, &cells);
    if let Some(&first) = record.windows.first() {
        window::focus(first);
    }
    let message = if missing == 0 {
        format!(
            "brought back {} ({} terminals)",
            fleet.name,
            record.windows.len()
        )
    } else if record.windows.len() < cells.len() {
        format!(
            "opened {} of {} terminals for {}: Windows Terminal did not show the rest in time",
            record.windows.len(),
            cells.len(),
            fleet.name
        )
    } else {
        format!(
            "opened {} ({} terminals on {})",
            fleet.name,
            record.windows.len(),
            m.device
        )
    };
    Ok((record, message))
}

/// Waits for `missing` new terminal windows and places each in the next free
/// cell the moment it shows, so a fleet assembles on screen rather than
/// appearing all at once in a heap and then jumping.
fn collect(record: &mut OpenFleet, before: &BTreeSet<u64>, missing: usize, cells: &[Rect]) {
    let target = record.windows.len() + missing;
    let start = Instant::now();
    while record.windows.len() < target && start.elapsed() < SHOW_TIMEOUT {
        for w in terminals() {
            if record.windows.len() >= target {
                break;
            }
            if before.contains(&w) || record.windows.contains(&w) || !window::is_shown(w) {
                continue;
            }
            let index = record.windows.len();
            record.windows.push(w);
            if let Some(&cell) = cells.get(index) {
                window::place(w, cell);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Puts every terminal of the fleet in its cell.
fn place_all(record: &OpenFleet, cells: &[Rect]) {
    for (&w, &cell) in record.windows.iter().zip(cells) {
        if window::frame(w) != Some(cell) {
            window::place(w, cell);
        }
    }
}

/// Puts an open fleet's terminals back in their cells without opening
/// anything.
pub fn snap(fleet: &Fleet, config: &Config, record: &mut OpenFleet) -> anyhow::Result<()> {
    record.prune();
    let (m, cells) = cells(fleet, config)?;
    record.monitor = m.device;
    place_all(record, &cells);
    Ok(())
}

/// The folder a fleet starts in: its configured one, else the user's profile.
///
/// `{n}` in it becomes the terminal's number, so each agent can work in a
/// folder of its own: `C:\src\site-{n}` for one git worktree per agent. A
/// folder that does not exist falls back to the profile folder rather than
/// failing the whole fleet.
fn directory(fleet: &Fleet, n: usize) -> PathBuf {
    fleet
        .directory
        .as_ref()
        .map(|d| numbered(d, n))
        .filter(|d| d.is_dir())
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| Path::new(r"C:\").to_path_buf())
}

/// A path with `{n}` replaced by `n`.
fn numbered(path: &Path, n: usize) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace("{n}", &n.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_numbered_directory_gives_each_terminal_its_own_folder() {
        assert_eq!(
            numbered(Path::new(r"C:\src\site-{n}"), 3),
            PathBuf::from(r"C:\src\site-3")
        );
        assert_eq!(
            numbered(Path::new(r"C:\src\site"), 3),
            PathBuf::from(r"C:\src\site")
        );
    }

    #[test]
    fn a_missing_numbered_folder_falls_back_instead_of_failing() {
        let fleet = Fleet {
            directory: Some(r"C:\no\such\place-{n}".into()),
            ..Fleet::default()
        };
        assert!(directory(&fleet, 1).is_dir());
    }
}
