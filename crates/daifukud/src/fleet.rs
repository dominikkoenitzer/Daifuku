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
pub const SHOW_TIMEOUT: Duration = Duration::from_secs(12);

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
    plan(
        fleet,
        config,
        stay_on,
        &monitor::monitors(),
        monitor::cursor(),
    )
    .ok_or_else(|| anyhow!("no monitor"))
}

/// Where a fleet's cells are on `monitors` with the cursor at `cursor`, as
/// [`cells`] works them out.
fn plan(
    fleet: &Fleet,
    config: &Config,
    stay_on: Option<&str>,
    monitors: &[MonitorInfo],
    cursor: (i32, i32),
) -> Option<(MonitorInfo, Vec<Rect>)> {
    let m = pick(monitors, &home_pick(&fleet.monitor, stay_on), cursor)?.clone();
    let cells = Grid::plan(fleet.count, m.work, config.gaps, fleet.shape);
    Some((m, cells))
}

/// Whether the monitors going from `before` to `now` moved an open fleet's
/// cells, or changed the scale of the monitor they are on, which makes
/// Windows resize its terminals. A change anywhere else, such as a taskbar
/// coming back on another monitor, leaves the fleet as it is.
pub fn moved(
    fleet: &Fleet,
    config: &Config,
    record: &OpenFleet,
    before: &[MonitorInfo],
    now: &[MonitorInfo],
    cursor: (i32, i32),
) -> bool {
    let on = |monitors: &[MonitorInfo]| {
        plan(fleet, config, Some(&record.monitor), monitors, cursor)
            .map(|(m, cells)| (m.dpi, cells))
    };
    on(before) != on(now)
}

/// The monitor to look for: a fleet that opened where the cursor was stays on
/// `stay_on`, the monitor it is on; any other goes where its config says.
fn home_pick(choice: &MonitorPick, stay_on: Option<&str>) -> MonitorPick {
    match (choice, stay_on) {
        (MonitorPick::Cursor, Some(device)) if !device.is_empty() => {
            MonitorPick::Device(device.to_owned())
        }
        (choice, _) => choice.clone(),
    }
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
    // Terminals past a lowered count are let go first, so they do not keep
    // the fleet on their monitor.
    record
        .slots
        .truncate(usize::try_from(fleet.count).unwrap_or(usize::MAX));
    let stay_on = record.windows().next().map(|_| record.monitor.clone());
    let (m, cells) = cells(fleet, config, stay_on.as_deref())?;
    settle(fleet, record, &m, cells.len());
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
                directory: Some(directory(fleet, n, elevated && !fleet.admin)),
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
            // The launch above runs at the daemon's level for an
            // administrator fleet and as the desktop user otherwise.
            match wait_for_new(&before, record, elevated && fleet.admin) {
                Some(w) => {
                    record.slots[slot] = Some(w);
                    window::place(w, cells[slot]);
                }
                None => break,
            }
        }
    }

    place_all(record, &cells, true);
    if let Some(first) = record.windows().next() {
        window::focus(first);
    }
    outcome(
        &fleet.name,
        &m.device,
        cells.len(),
        !empty.is_empty(),
        record.windows().count(),
    )
}

/// What opening fleet `name` with `cells` cells on `device` says, with
/// `open` of its terminals open now and whether any had to be `started`.
/// No terminal at all is a failure, so a key that opened nothing says so.
/// A terminal that shows after its wait is not the fleet's, and opening the
/// fleet again would start another in its place: the message says so.
fn outcome(
    name: &str,
    device: &str,
    cells: usize,
    started: bool,
    open: usize,
) -> anyhow::Result<String> {
    const LATE: &str = "close any that show up late, they are not part of the fleet";
    if !started {
        return Ok(format!("brought back {name} ({})", terminals_count(open)));
    }
    if open == 0 {
        return Err(anyhow!(
            "Windows Terminal did not show a terminal for {name} in time; {LATE}"
        ));
    }
    if open < cells {
        return Ok(format!(
            "opened {open} of {} for {name}: Windows Terminal did not show the rest in time; {LATE}",
            terminals_count(cells)
        ));
    }
    Ok(format!(
        "opened {name} ({} on {device})",
        terminals_count(open)
    ))
}

/// Brings a record in line with where its fleet is now.
///
/// The config's count may have changed since the fleet opened: slots past it
/// are let go (their terminals stay open, unmanaged), missing ones added. A
/// fleet that opened where the cursor was keeps that monitor as its home
/// while it is gone, so it returns there when the monitor comes back.
fn settle(fleet: &Fleet, record: &mut OpenFleet, m: &MonitorInfo, count: usize) {
    let home_gone = fleet.monitor == MonitorPick::Cursor
        && record.windows().next().is_some()
        && !record.monitor.is_empty()
        && !record.monitor.eq_ignore_ascii_case(&m.device);
    if !home_gone {
        record.monitor.clone_from(&m.device);
    }
    record.slots.resize(count, None);
}

/// Waits for the terminal window a launch `elevated` or not opens, as
/// [`launched`] tells it, until it takes messages, so it is placed and
/// measured like any other window rather than by a queued move.
fn wait_for_new(before: &BTreeSet<u64>, record: &OpenFleet, elevated: bool) -> Option<u64> {
    let start = Instant::now();
    while start.elapsed() < SHOW_TIMEOUT {
        if let Some(w) = terminals().into_iter().find(|&w| {
            launched(w, before, record, elevated, window::is_shown, |w| {
                daifuku_win::process::is_elevated(window::pid(w))
            })
        }) {
            while !window::answers(w) && start.elapsed() < SHOW_TIMEOUT {
                std::thread::sleep(Duration::from_millis(25));
            }
            return Some(w);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Whether terminal window `w` can be the one a launch `elevated` or not
/// just opened: not there before, in none of the fleet's slots, shown, and
/// running as elevated as the launch. A terminal of the other kind that
/// shows meanwhile, such as one a person opens, is not taken for it.
fn launched(
    w: u64,
    before: &BTreeSet<u64>,
    record: &OpenFleet,
    elevated: bool,
    shown: impl Fn(u64) -> bool,
    elevation: impl Fn(u64) -> Option<bool>,
) -> bool {
    !before.contains(&w)
        && !record.slots.contains(&Some(w))
        && shown(w)
        && elevation(w) == Some(elevated)
}

/// Puts every terminal of the fleet in its cell. A minimised or maximised one
/// is restored into it only when `restore` is set, as it is when someone asks
/// for the fleet; otherwise it is left as they put it.
fn place_all(record: &OpenFleet, cells: &[Rect], restore: bool) {
    for (slot, &cell) in record.slots.iter().zip(cells) {
        if let Some(w) = *slot
            && (restore || !(window::is_minimised(w) || window::is_maximised(w)))
            && window::frame(w) != Some(cell)
        {
            window::place(w, cell);
        }
    }
}

/// Puts an open fleet's terminals back in their cells without opening
/// anything. A minimised or maximised terminal is restored into its cell
/// only when `restore` is set.
pub fn snap(
    fleet: &Fleet,
    config: &Config,
    record: &mut OpenFleet,
    restore: bool,
) -> anyhow::Result<()> {
    record.prune();
    record
        .slots
        .truncate(usize::try_from(fleet.count).unwrap_or(usize::MAX));
    let (m, cells) = cells(fleet, config, Some(&record.monitor))?;
    settle(fleet, record, &m, cells.len());
    place_all(record, &cells, restore);
    Ok(())
}

/// `1 terminal`, `4 terminals`.
pub fn terminals_count(n: usize) -> String {
    if n == 1 {
        "1 terminal".to_owned()
    } else {
        format!("{n} terminals")
    }
}

/// The folder a fleet starts in: its configured one, else the user's profile.
///
/// `{n}` in it becomes the terminal's number, so each agent can work in a
/// folder of its own: `C:\src\site-{n}` for one git worktree per agent. A
/// folder that does not exist falls back to the profile folder rather than
/// failing the whole fleet.
///
/// `as_user` is set for a terminal started as the desktop user from an
/// elevated daemon. Drive letters belong to a sign-in, and an elevated
/// process has one of its own, so a `subst` or mapped drive the user's
/// terminal sees may not exist for the daemon: the folder is looked for as
/// the user.
fn directory(fleet: &Fleet, n: usize, as_user: bool) -> PathBuf {
    let exists = |d: &PathBuf| {
        if as_user {
            daifuku_win::process::as_shell_user(|| d.is_dir()).unwrap_or_else(|_| d.is_dir())
        } else {
            d.is_dir()
        }
    };
    fleet
        .directory
        .as_ref()
        .map(|d| numbered(d, n))
        .filter(exists)
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
    fn opening_no_terminal_at_all_fails() {
        let d = r"\\.\DISPLAY1";
        assert!(outcome("agents", d, 6, true, 0).is_err());
        assert_eq!(
            outcome("agents", d, 6, true, 2).unwrap(),
            "opened 2 of 6 terminals for agents: Windows Terminal did not show the rest in time; close any that show up late, they are not part of the fleet"
        );
        assert_eq!(
            outcome("agents", d, 6, true, 6).unwrap(),
            r"opened agents (6 terminals on \\.\DISPLAY1)"
        );
        assert_eq!(
            outcome("agents", d, 1, false, 1).unwrap(),
            "brought back agents (1 terminal)"
        );
    }

    #[test]
    fn only_a_new_shown_terminal_of_the_launch_s_kind_is_taken() {
        let before = BTreeSet::from([1]);
        let r = record("", vec![Some(2), None]);
        let elevated = |w| match w {
            3..=5 => Some(true),
            6 => Some(false),
            _ => None,
        };
        let shown = |w| w != 4;
        let take = |w, admin| launched(w, &before, &r, admin, shown, elevated);
        assert!(take(3, true), "new, shown and elevated");
        assert!(!take(1, true), "there before");
        assert!(!take(2, true), "already in the fleet");
        assert!(!take(4, true), "not shown yet");
        assert!(!take(6, true), "a terminal of the user's own");
        assert!(take(6, false), "the user's, for an unelevated fleet");
        assert!(!take(3, false), "an administrator's");
        assert!(!take(7, false), "one that cannot be asked");
    }

    #[test]
    fn a_closed_terminal_leaves_its_slot_empty_and_the_rest_in_place() {
        let mut record = OpenFleet::new("agents");
        record.slots = vec![Some(1), Some(2), Some(3)];
        record.forget(2);
        assert_eq!(record.slots, vec![Some(1), None, Some(3)]);
        assert_eq!(record.windows().collect::<Vec<_>>(), vec![1, 3]);
    }

    #[test]
    fn a_missing_numbered_folder_falls_back_instead_of_failing() {
        let fleet = Fleet {
            directory: Some(r"C:\no\such\place-{n}".into()),
            ..Fleet::default()
        };
        assert!(directory(&fleet, 1, false).is_dir());
        assert!(directory(&fleet, 1, true).is_dir());
    }

    #[test]
    fn a_folder_the_user_can_see_is_kept_when_looked_for_as_the_user() {
        let here = std::env::current_dir().unwrap();
        let fleet = Fleet {
            directory: Some(here.clone()),
            ..Fleet::default()
        };
        assert_eq!(directory(&fleet, 1, true), here);
        assert_eq!(directory(&fleet, 1, false), here);
    }

    fn on(device: &str) -> MonitorInfo {
        let r = Rect::new(0, 0, 1920, 1080);
        MonitorInfo {
            device: device.to_owned(),
            bounds: r,
            work: r,
            dpi: 96,
            primary: false,
        }
    }

    fn fleet_on(monitor: MonitorPick) -> Fleet {
        Fleet {
            monitor,
            ..Fleet::default()
        }
    }

    fn record(monitor: &str, slots: Vec<Option<u64>>) -> OpenFleet {
        OpenFleet {
            name: "agents".into(),
            monitor: monitor.into(),
            slots,
        }
    }

    #[test]
    fn a_cursor_fleet_keeps_its_home_while_that_monitor_is_gone() {
        let mut r = record(r"\\.\DISPLAY1", vec![Some(1), None]);
        settle(
            &fleet_on(MonitorPick::Cursor),
            &mut r,
            &on(r"\\.\DISPLAY2"),
            2,
        );
        assert_eq!(r.monitor, r"\\.\DISPLAY1");
    }

    #[test]
    fn a_cursor_fleet_with_no_terminals_or_no_home_takes_the_current_monitor() {
        let cursor = fleet_on(MonitorPick::Cursor);
        for mut r in [
            record(r"\\.\DISPLAY1", vec![None, None]),
            record(r"\\.\DISPLAY1", Vec::new()),
            record("", vec![Some(1)]),
        ] {
            settle(&cursor, &mut r, &on(r"\\.\DISPLAY2"), 2);
            assert_eq!(r.monitor, r"\\.\DISPLAY2", "{:?}", r.slots);
        }
    }

    #[test]
    fn a_fleet_on_any_other_monitor_takes_the_current_one() {
        for pick in [
            MonitorPick::Portrait,
            MonitorPick::Primary,
            MonitorPick::Device(r"\\.\DISPLAY1".into()),
        ] {
            let mut r = record(r"\\.\DISPLAY1", vec![Some(1)]);
            settle(&fleet_on(pick.clone()), &mut r, &on(r"\\.\DISPLAY2"), 1);
            assert_eq!(r.monitor, r"\\.\DISPLAY2", "{pick:?}");
        }
    }

    #[test]
    fn a_cursor_fleet_home_matches_its_monitor_whatever_the_case() {
        let mut r = record(r"\\.\display2", vec![Some(1)]);
        settle(
            &fleet_on(MonitorPick::Cursor),
            &mut r,
            &on(r"\\.\DISPLAY2"),
            1,
        );
        assert_eq!(
            r.monitor, r"\\.\DISPLAY2",
            "the same monitor, as spelled now"
        );
    }

    #[test]
    fn a_monitor_change_moves_only_the_fleets_it_touches() {
        let wide = Rect::new(0, 0, 2560, 1440);
        let tall = Rect::new(2560, 0, 3640, 1920);
        let left = |work| MonitorInfo {
            device: r"\\.\DISPLAY1".into(),
            bounds: wide,
            work,
            dpi: 96,
            primary: true,
        };
        let right = |work, dpi| MonitorInfo {
            device: r"\\.\DISPLAY2".into(),
            bounds: tall,
            work,
            dpi,
            primary: false,
        };
        let (left_bar, right_bar) = (Rect::new(0, 0, 2560, 1392), Rect::new(2560, 0, 3640, 1872));
        let both = [left(left_bar), right(right_bar, 96)];
        let config = Config::default();
        let on_right = record(r"\\.\DISPLAY2", vec![Some(1)]);
        let cursor = (100, 100);
        for pick in [
            MonitorPick::Portrait,
            MonitorPick::Cursor,
            MonitorPick::Device(r"\\.\DISPLAY2".into()),
        ] {
            let fleet = fleet_on(pick.clone());
            let moves = |before: &[MonitorInfo], now: &[MonitorInfo]| {
                moved(&fleet, &config, &on_right, before, now, cursor)
            };
            assert!(
                !moves(&both, &[left(wide), right(right_bar, 96)]),
                "{pick:?}: the other monitor's taskbar went"
            );
            assert!(
                moves(&both, &[left(left_bar), right(tall, 96)]),
                "{pick:?}: its own taskbar went"
            );
            assert!(
                moves(&both, &[left(left_bar), right(right_bar, 144)]),
                "{pick:?}: its monitor's scale changed"
            );
            assert!(
                moves(&both, &[left(left_bar)]),
                "{pick:?}: its monitor went"
            );
            assert!(
                moves(&[left(left_bar)], &both),
                "{pick:?}: its monitor came back"
            );
        }
    }

    #[test]
    fn settling_fits_the_slots_to_the_count() {
        let fleet = fleet_on(MonitorPick::Portrait);
        let mut r = record("", vec![Some(1), None, Some(3)]);
        settle(&fleet, &mut r, &on(r"\\.\DISPLAY1"), 2);
        assert_eq!(r.slots, vec![Some(1), None]);
        settle(&fleet, &mut r, &on(r"\\.\DISPLAY1"), 4);
        assert_eq!(r.slots, vec![Some(1), None, None, None]);
    }

    #[test]
    fn only_a_cursor_fleet_with_a_home_looks_for_it() {
        let home = r"\\.\DISPLAY2";
        assert_eq!(
            home_pick(&MonitorPick::Cursor, Some(home)),
            MonitorPick::Device(home.into())
        );
        assert_eq!(
            home_pick(&MonitorPick::Cursor, Some("")),
            MonitorPick::Cursor
        );
        assert_eq!(home_pick(&MonitorPick::Cursor, None), MonitorPick::Cursor);
        assert_eq!(
            home_pick(&MonitorPick::Portrait, Some(home)),
            MonitorPick::Portrait
        );
        let own = MonitorPick::Device(r"\\.\DISPLAY1".into());
        assert_eq!(home_pick(&own, Some(home)), own);
    }
}
