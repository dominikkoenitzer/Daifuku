//! Opening fleets and keeping their terminals in their cells.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use daifuku_core::Rect;
use daifuku_core::config::{Config, Fleet, MonitorPick};
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
    /// One slot per cell, in cell order: the terminal in it, or `None` when
    /// that terminal was closed. A slot keeps its place, so a terminal that is
    /// opened again lands in the cell, and gets the `{n}`, of the one it
    /// replaces, while every other terminal stays where it is.
    pub slots: Vec<Option<u64>>,
}

impl OpenFleet {
    /// A fleet with no terminals yet.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            monitor: String::new(),
            slots: Vec::new(),
        }
    }

    /// Empties the slots whose windows no longer exist or stopped being
    /// terminals (a handle reused by some other window).
    pub fn prune(&mut self) {
        for slot in &mut self.slots {
            if slot.is_some_and(|w| !(window::exists(w) && window::class(w) == WINDOW_CLASS)) {
                *slot = None;
            }
        }
    }

    /// The terminals that are open, in cell order.
    pub fn windows(&self) -> impl Iterator<Item = u64> + '_ {
        self.slots.iter().flatten().copied()
    }

    /// Forgets a window that was destroyed.
    pub fn forget(&mut self, w: u64) {
        for slot in &mut self.slots {
            if *slot == Some(w) {
                *slot = None;
            }
        }
    }
}

/// Where a fleet's cells are right now, with the monitor recomputed so a fleet
/// follows a monitor that was unplugged and plugged back.
///
/// `stay_on` is the monitor the fleet is on, when it is already open. A fleet
/// that opens where the cursor is stays on that monitor afterwards: snapping
/// it back must not carry it to wherever the cursor happens to be now.
fn cells(
    fleet: &Fleet,
    config: &Config,
    stay_on: Option<&str>,
) -> anyhow::Result<(MonitorInfo, Vec<Rect>)> {
    let monitors = monitor::monitors();
    let choice = match (&fleet.monitor, stay_on) {
        (MonitorPick::Cursor, Some(device)) if !device.is_empty() => {
            MonitorPick::Device(device.to_owned())
        }
        (choice, _) => choice.clone(),
    };
    let m = pick(&monitors, &choice, monitor::cursor())
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
/// `record` is the fleet's record, empty when it is not open yet. It holds
/// every terminal opened so far even when this fails part way, so none is
/// lost. Returns a line for the person who asked.
pub fn open(
    fleet: &Fleet,
    config: &Config,
    record: &mut OpenFleet,
    elevated: bool,
) -> anyhow::Result<String> {
    record.prune();
    let stay_on = record.windows().next().map(|_| record.monitor.clone());
    let (m, cells) = cells(fleet, config, stay_on.as_deref())?;
    record.monitor = m.device.clone();
    // The config's count may have changed since the fleet opened: slots past
    // it are let go (their terminals stay open, unmanaged), missing ones added.
    record.slots.resize(cells.len(), None);
    let empty: Vec<usize> = (0..cells.len())
        .filter(|&i| record.slots[i].is_none())
        .collect();

    if !empty.is_empty() {
        if fleet.admin && !elevated {
            return Err(anyhow!(
                "fleet `{}` opens administrator terminals, and this daemon is not elevated. Install Daifuku, or set \"admin\": false",
                fleet.name
            ));
        }
        let wt = terminal::find().context("Windows Terminal is not installed")?;
        // One terminal at a time, each waited for and put in its own cell:
        // Windows Terminal shows windows in no promised order, so opening them
        // all at once could give a terminal the cell, and the `{n}`, meant
        // for another.
        for &slot in &empty {
            let n = slot + 1;
            let launch = Launch {
                directory: Some(directory(fleet, n)),
                profile: fleet.profile.clone(),
                command: fleet
                    .command
                    .as_ref()
                    .map(|c| c.replace("{n}", &n.to_string())),
                title: Some(format!("{} {}", fleet.name, n)),
                clean: fleet.no_profile,
            };
            let before = terminals();
            let r = if fleet.admin || !elevated {
                terminal::open(&wt, &launch)
            } else {
                terminal::open_unelevated(&wt, &launch)
            };
            r.with_context(|| format!("could not start Windows Terminal at {}", wt.display()))?;
            match wait_for_new(&before, record) {
                Some(w) => {
                    record.slots[slot] = Some(w);
                    window::place(w, cells[slot]);
                }
                None => break,
            }
        }
    }

    place_all(record, &cells);
    if let Some(first) = record.windows().next() {
        window::focus(first);
    }
    let open_now = record.windows().count();
    let message = if empty.is_empty() {
        format!("brought back {} ({open_now} terminals)", fleet.name)
    } else if open_now < cells.len() {
        format!(
            "opened {open_now} of {} terminals for {}: Windows Terminal did not show the rest in time",
            cells.len(),
            fleet.name
        )
    } else {
        format!(
            "opened {} ({open_now} terminals on {})",
            fleet.name, m.device
        )
    };
    Ok(message)
}

/// Waits for one new terminal window: shown, not there before, and not
/// already one of the fleet's.
fn wait_for_new(before: &BTreeSet<u64>, record: &OpenFleet) -> Option<u64> {
    let start = Instant::now();
    while start.elapsed() < SHOW_TIMEOUT {
        if let Some(w) = terminals().into_iter().find(|w| {
            !before.contains(w) && !record.slots.contains(&Some(*w)) && window::is_shown(*w)
        }) {
            return Some(w);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Puts every terminal of the fleet in its cell.
fn place_all(record: &OpenFleet, cells: &[Rect]) {
    for (slot, &cell) in record.slots.iter().zip(cells) {
        if let Some(w) = *slot
            && window::frame(w) != Some(cell)
        {
            window::place(w, cell);
        }
    }
}

/// Puts an open fleet's terminals back in their cells without opening
/// anything.
pub fn snap(fleet: &Fleet, config: &Config, record: &mut OpenFleet) -> anyhow::Result<()> {
    record.prune();
    let (m, cells) = cells(fleet, config, Some(&record.monitor))?;
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
