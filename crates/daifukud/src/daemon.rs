//! The daemon's state and its message loop.

use std::path::PathBuf;

use anyhow::Context;
use daifuku_core::config::{Border, Config};
use daifuku_core::peer::{self, Kept, KeptFile, Spawn};
use daifuku_core::protocol::{
    AgentWindow, FleetStatus, HookMessage, PeerStatus, Request, Response, RestoredPeer, Status,
};
use daifuku_core::saved::{self, Saved, SavedFleet, SavedShown};
use daifuku_core::state::{AgentState, Agents};
use daifuku_render::{BorderConfig, BorderManager, BorderSpec, WindowHandle};
use daifuku_win::terminal::WINDOW_CLASS;
use daifuku_win::{access, clock, dpi, paths, process, window};
use windows::Win32::UI::Shell::{QUNS_NOT_PRESENT, SHQueryUserNotificationState};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, MSG, PostQuitMessage, SetTimer, TranslateMessage,
    WM_HOTKEY, WM_TIMER,
};

use crate::events::{self, Event};
use crate::fleet::{self, OpenFleet};
use crate::hotkeys::{Action, Hotkeys};
use crate::ipc::{self, Inbound, Inbox, WM_INBOX};

/// The most sessions tracked at once. A hook can report any session id it
/// likes; past this, new ones are ignored instead of growing the map forever.
const MAX_SESSIONS: usize = 512;

/// The built-in demo fleet's name.
const DEMO: &str = "demo";

/// How often dead windows are swept, in milliseconds. Destroy events do most
/// of the work; this catches the ones Windows did not deliver.
const SWEEP_MS: u32 = 2000;

/// How long `close` waits for its terminals to go. Windows Terminal closes a
/// window at once, unless it asks first whether to close all its tabs.
const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How often a waiting border's pulse repaints: twenty times a second, which
/// is smooth for a slow breath and nothing for the compositor.
const PULSE_MS: u32 = 50;

/// One breath of the pulse, in seconds.
const PULSE_PERIOD: f64 = 1.6;

/// How often the state is saved again when nothing changed, in 100 ns ticks:
/// often enough that a daemon restarted on an error finds it fresh.
const RESAVE_TICKS: u64 = 60 * 10_000_000;

/// A peer `daifuku spawn` opened.
struct Peer {
    /// Its terminal.
    window: u64,
    /// What it was opened with.
    spawn: Spawn,
}

struct Daemon {
    config: Config,
    config_path: PathBuf,
    config_error: Option<String>,
    agents: Agents<u64>,
    fleets: Vec<OpenFleet>,
    /// Every peer `daifuku spawn` opened that is still open.
    peers: Vec<Peer>,
    /// The agents' own processes, as their hooks name them, and the sessions
    /// each one reported. An agent whose process ended is gone even when its
    /// terminal stays open, as a Windows Terminal tab does when its shell is
    /// killed: nothing else would tell the daemon.
    agent_processes:
        std::collections::HashMap<u32, (process::Watch, std::collections::BTreeSet<String>)>,
    /// Labels `daifuku label` gave terminals that are not peers, whose
    /// labels live in their [`Spawn`].
    labels: std::collections::HashMap<u64, String>,
    /// Peers that were open when Windows last stopped, for `daifuku restore`
    /// to open again.
    dormant: Vec<Spawn>,
    /// The start of Windows the dormant peers were open in.
    dormant_boot: u64,
    /// Where the peers are kept across a restart of Windows, `None` when
    /// they are not: only the installed daemon keeps them, as with
    /// [`Daemon::state_file`].
    peers_file: Option<PathBuf>,
    borders: Option<BorderManager>,
    hotkeys: Hotkeys,
    elevated: bool,
    /// Windows' own settings, read at start and on every sweep.
    high_contrast: bool,
    animations: bool,
    /// Whether nobody is at the desktop, read on every sweep: see [`away()`].
    away: bool,
    /// The pulse timer, running only while an agent waits.
    pulse_timer: Option<usize>,
    /// The window event hooks. The ones borders follow their windows by are
    /// in only while an agent has a window and borders are drawn.
    hooks: events::Hooks,
    /// Every agent window with a frame on screen, its state and its frame,
    /// as the last full pass measured them.
    framed: Vec<(u64, AgentState, daifuku_core::Rect)>,
    started: std::time::Instant,
    /// When the config file last changed, to reload it without being asked.
    config_stamp: Option<std::time::SystemTime>,
    /// The stamp of the config file when reading it last failed, so each
    /// version of the file is read at most twice.
    config_failed: Option<std::time::SystemTime>,
    /// The monitors as last seen, to put fleets back when they change.
    monitors: Vec<daifuku_core::monitor::MonitorInfo>,
    /// Since when each session has shown its state in the window it runs
    /// in, for `daifuku status` and `next`: see [`window_since`].
    shown_since: Shown,
    /// The terminal `next` last brought to the front, and since when its
    /// agent had shown the state it was brought up for, so that pressing it
    /// again from there moves on instead of starting over.
    last_next: Option<(u64, std::time::Instant)>,
    /// Where the state is kept across a restart, `None` when it is not: see
    /// [`daifuku_core::saved`].
    state_file: Option<PathBuf>,
    /// The state file versions up to 0.1.3 shared between users, read when
    /// this user's own does not exist yet.
    legacy_state_file: Option<PathBuf>,
    /// What the last save held, apart from when it was made, and the
    /// interrupt time it was made at.
    last_saved: (String, u64),
}

/// Runs the daemon until it is told to stop.
///
/// Logging is set up by the caller, so an error this returns still reaches
/// the log before the writer stops.
///
/// `state_dir` keeps the saved state and the peers in that folder, for a
/// daemon on a config of its own, as the end-to-end tests run theirs.
pub fn run(config_override: Option<PathBuf>, state_dir: Option<PathBuf>) -> anyhow::Result<()> {
    dpi::per_monitor_v2();
    let elevated = process::current_is_elevated();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        elevated,
        "daifukud starting"
    );

    // SAFETY: no arguments.
    let main_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    let inbox = ipc::start(main_thread).context("could not open Daifuku's pipes")?;

    // Only the installed daemon, elevated and on its own config, keeps its
    // state: one run on another config, as the end-to-end tests run theirs,
    // starts clean every time and leaves the installed one's state alone.
    let keeps_state = elevated && config_override.is_none();
    let config_path = config_override
        .or_else(paths::config_file)
        .context("no ProgramData folder")?;
    let mut d = Daemon::new(config_path, elevated, events::Hooks::install());
    if let Some(dir) = state_dir.or_else(|| paths::data_dir().filter(|_| keeps_state)) {
        let files = SavedFiles::in_dir(&dir, daifuku_win::setup::user_sid().as_deref());
        d.state_file = Some(files.state);
        d.peers_file = Some(files.peers);
        d.legacy_state_file = files.legacy_state;
    }
    d.load_config();
    d.borders = BorderManager::new(BorderConfig::from(&d.config.border))
        .inspect_err(|e| tracing::error!(error = %e, "no borders"))
        .ok();
    d.hotkeys.register(&d.config);
    d.restore();
    d.load_peers();
    d.refresh_borders();
    // SAFETY: a thread timer, killed before returning.
    let timer = unsafe { SetTimer(None, 0, SWEEP_MS, None) };

    let mut msg = MSG::default();
    loop {
        // SAFETY: msg is a valid out pointer; this thread owns its queue.
        let got = unsafe { GetMessageW(&raw mut msg, None, 0, 0) };
        if got.0 <= 0 {
            break;
        }
        match msg.message {
            WM_HOTKEY => {
                let id = i32::try_from(msg.wParam.0).unwrap_or(-1);
                if let Some(action) = d.hotkeys.action(id) {
                    d.hotkey(&action);
                }
            }
            WM_INBOX => d.inbox(&inbox),
            // Nothing to do here: window_events() below drains the queue.
            events::WM_EVENTS => {}
            WM_TIMER if msg.hwnd.is_invalid() => {
                if Some(msg.wParam.0) == d.pulse_timer {
                    d.paint_borders();
                } else {
                    d.sweep();
                }
            }
            _ => {
                // SAFETY: standard dispatch of a message this thread received.
                unsafe {
                    let _ = TranslateMessage(&raw const msg);
                    DispatchMessageW(&raw const msg);
                }
            }
        }
        d.window_events();
    }

    // SAFETY: the timer this thread set.
    unsafe {
        let _ = KillTimer(None, timer);
    }
    d.hotkeys.unregister();
    // An install stops the daemon to replace it: the next one takes up its
    // fleets and agents from here.
    d.save(true);
    if let Some(borders) = d.borders.take() {
        borders.stop();
    }
    // `daifuku stop` waits for its "stopping"; exiting first would cut it off.
    inbox.wait_for_stop_reply(std::time::Duration::from_secs(1));
    tracing::info!("daifukud stopped");
    Ok(())
}

impl Daemon {
    /// A daemon on the default config, with no agents, fleets, borders or
    /// hotkeys yet, that reads its config from `config_path`.
    fn new(config_path: PathBuf, elevated: bool, hooks: events::Hooks) -> Self {
        Self {
            config: Config::default(),
            config_path,
            config_error: None,
            agents: Agents::new(),
            fleets: Vec::new(),
            peers: Vec::new(),
            agent_processes: std::collections::HashMap::new(),
            labels: std::collections::HashMap::new(),
            dormant: Vec::new(),
            dormant_boot: 0,
            peers_file: None,
            borders: None,
            hotkeys: Hotkeys::default(),
            elevated,
            high_contrast: access::high_contrast(),
            animations: access::animations(),
            away: away(),
            pulse_timer: None,
            hooks,
            framed: Vec::new(),
            started: std::time::Instant::now(),
            config_stamp: None,
            config_failed: None,
            monitors: daifuku_win::monitor::monitors(),
            shown_since: std::collections::HashMap::new(),
            last_next: None,
            state_file: None,
            legacy_state_file: None,
            last_saved: (String::new(), 0),
        }
    }

    fn load_config(&mut self) {
        self.config_stamp = stamp(&self.config_path);
        let text = match std::fs::read(&self.config_path)
            .and_then(|b| daifuku_core::config::decode(&b))
        {
            Ok(t) => {
                self.config_failed = None;
                t
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                self.config_error = Some(format!("{}: {e}", self.config_path.display()));
                tracing::error!(path = %self.config_path.display(), error = %e, "config unreadable, keeping the last good one");
                retry_once(&mut self.config_failed, &mut self.config_stamp);
                return;
            }
        };
        match Config::from_json(&text) {
            Ok(c) => {
                self.config = c;
                self.config_error = None;
                tracing::info!(path = %self.config_path.display(), fleets = self.config.fleets.len(), "config loaded");
            }
            Err(e) => {
                self.config_error = Some(e.to_string());
                tracing::error!(error = %e, "config invalid, keeping the last good one");
            }
        }
    }

    /// Whether the config in use is the built-in one, as it is when the
    /// daemon started with a config file it could not use.
    fn on_defaults(&self) -> bool {
        self.config == Config::default()
    }

    fn hotkey(&mut self, action: &Action) {
        tracing::info!(?action, "hotkey");
        // Close leaves no fleet open, so whether it had one to act on is
        // known only beforehand.
        let had_nothing = self.idle(action);
        let result = match action {
            Action::Open(name) => self.open(Some(name)),
            Action::Next => self.next(),
            Action::Snap => self.snap(),
            Action::Close => self.close_all(),
        };
        let idle = if *action == Action::Close {
            had_nothing
        } else {
            self.idle(action)
        };
        match &result {
            Response::Error { message } => tracing::warn!(%message, "hotkey failed"),
            Response::Ok {
                message: Some(message),
            } if idle => tracing::info!(%message, "hotkey had nothing to do"),
            _ => {}
        }
        if idle || matches!(result, Response::Error { .. }) {
            // A key that did nothing says so; the reason is in the log.
            access::refused();
        }
    }

    /// Whether a key had nothing to act on: no agent waits for `next`, no
    /// fleet is open for `snap` or `close`.
    fn idle(&self, action: &Action) -> bool {
        match action {
            Action::Open(_) => false,
            Action::Next => self.waiting().is_empty(),
            Action::Snap | Action::Close => self.fleets.is_empty(),
        }
    }

    fn inbox(&mut self, inbox: &Inbox) {
        while let Ok(item) = inbox.receiver.try_recv() {
            match item {
                Inbound::Hook(message, sender) => self.hook(&message, &sender),
                Inbound::Control(request, reply, claimed) => {
                    // Withdrawn while it waited behind something long: its
                    // client was told nothing was done, so nothing is.
                    if !ipc::claim(&claimed) {
                        tracing::info!(?request, "dropped a command its client gave up on");
                        continue;
                    }
                    let response = self.control(&request);
                    let _ = reply.send(response);
                    if matches!(request, Request::Stop) {
                        // SAFETY: ends this thread's message loop.
                        unsafe { PostQuitMessage(0) };
                    }
                }
            }
        }
    }

    fn control(&mut self, request: &Request) -> Response {
        match request {
            Request::Open { fleet } => self.open(fleet.as_deref()),
            Request::Snap => self.snap(),
            Request::Close { fleet } => self.close(fleet.as_deref()),
            Request::CloseAll => self.close_all(),
            Request::Next => self.next(),
            Request::Demo => self.demo(),
            Request::Spawn(spawn) => self.spawn(spawn),
            Request::Label { window, label } => self.label(*window, label.as_deref()),
            Request::Restore { dry_run, forget } => self.restore_peers(*dry_run, *forget),
            Request::Status => Response::Status(self.status()),
            Request::Reload => {
                self.load_config();
                self.hotkeys.register(&self.config);
                if let Some(b) = &self.borders {
                    let _ = b.set_config(BorderConfig::from(&self.config.border));
                }
                self.refresh_borders();
                match &self.config_error {
                    Some(e) if self.on_defaults() => {
                        Response::error(format!("kept the defaults: {e}"))
                    }
                    Some(e) => Response::error(format!("kept the previous config: {e}")),
                    None => Response::said("config reloaded"),
                }
            }
            Request::Stop => Response::said("stopping"),
        }
    }

    /// One hook report. `sender` is the pipe client's process and its
    /// ancestors, as the pipe told them, which the agent's own process is
    /// trusted from: a process the message names is the agent's only when it
    /// is one of them.
    fn hook(&mut self, message: &HookMessage, sender: &[u32]) {
        // Only a live, visible top-level window gets a frame. A pseudo console
        // window, which a hook names when its terminal was not ready yet, is
        // followed to the terminal window now; anything else is a stale handle
        // or a made-up one.
        let Some(w) = daifuku_win::console::visible_window(message.window) else {
            tracing::debug!(
                window = format!("{:#x}", message.window),
                "hook named no window to draw around"
            );
            return;
        };
        if self.agents.window_state(w).is_none() {
            // The first report from a window says which window the hook
            // resolved to, which is the one thing a hook can get wrong.
            tracing::debug!(
                window = format!("{w:#x}"),
                class = %window::class(w),
                title = %window::title(w),
                shown = window::is_shown(w),
                frame = ?window::frame(w),
                "first report from a window"
            );
        }
        if self.agents.len() >= MAX_SESSIONS && !self.agents.knows(&message.event.session_id) {
            return;
        }
        // By session, not by window: a second tab that starts waiting in a
        // window that waits already changes nothing on screen, yet it is one
        // more agent that needs you.
        let session = &message.event.session_id;
        let agent = trusted_agent(message.pid, sender);
        self.note_peer_session(w, session, agent);
        let before = self.agents.session_state(session);
        let changed = self.agents.apply(w, &message.event);
        let now = self.agents.session_state(session);
        if changed || now != before {
            self.note_states();
        }
        if let Some(pid) = agent
            && now.is_some()
        {
            self.watch_agent(pid, session);
        }
        if self.config.sound && now == Some(AgentState::Waiting) && before != now {
            access::chime();
        }
        if changed {
            tracing::debug!(window = format!("{w:#x}"), event = %message.event.hook_event_name, state = ?self.agents.window_state(w), "agent state");
            self.refresh_borders();
        }
    }

    /// Keeps `shown_since` in step with every session's state. A session
    /// that moves to another window starts its time there anew.
    fn note_states(&mut self) {
        note_shown(
            &mut self.shown_since,
            self.agents
                .sessions()
                .map(|(id, w, state)| ((id.to_owned(), w), state))
                .collect(),
            std::time::Instant::now(),
        );
    }

    fn open(&mut self, name: Option<&str>) -> Response {
        let fleet = match name {
            Some(n) => self.config.fleet(n).cloned(),
            None => self.config.fleets.first().cloned(),
        };
        let Some(fleet) = fleet else {
            return Response::error(match name {
                Some(n) => format!("no fleet called `{n}` in {}", self.config_path.display()),
                None => "the config has no fleets".to_owned(),
            });
        };
        let mut said = Vec::new();
        let mut failed = Vec::new();
        for other in &opening_order(&self.config, fleet) {
            match self.open_fleet(other) {
                Response::Error { message } => failed.push(message),
                Response::Ok {
                    message: Some(message),
                } => said.push(message),
                _ => {}
            }
        }
        if failed.is_empty() {
            Response::said(said.join("; "))
        } else {
            Response::error(failed.join("; "))
        }
    }

    /// Opens a fleet, or brings back the one that is open. Its record goes
    /// back into the list whatever happens, so a failure part way loses
    /// neither the terminals that were open nor the ones just started.
    fn open_fleet(&mut self, fleet: &daifuku_core::config::Fleet) -> Response {
        let mut record = match self
            .fleets
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&fleet.name))
        {
            Some(i) => self.fleets.remove(i),
            None => OpenFleet::new(&fleet.name),
        };
        let result = fleet::open(fleet, &self.config, &mut record, self.elevated);
        if record.windows().next().is_some() {
            self.fleets.push(record);
        }
        self.refresh_borders();
        match result {
            Ok(message) => {
                tracing::info!(%message);
                Response::said(message)
            }
            Err(e) => Response::error(format!("{e:#}")),
        }
    }

    /// Six scripted agents in a clean fleet on the first fleet's monitor.
    /// The script is `daifuku demo-agent`, from the folder this daemon runs
    /// from, so an installed daemon only ever starts an installed binary.
    fn demo(&mut self) -> Response {
        // Fleet names match in any case, so a configured fleet called `Demo`
        // would share its record with the demo's terminals.
        if self.config.fleet(DEMO).is_some() {
            return Response::error(format!(
                "the config has a fleet called `{DEMO}`: rename it to run the demo"
            ));
        }
        // Checked here, not in `demo_fleet`, which snap uses too: an open
        // demo still snaps when the file has gone since.
        let Some(fleet) = self
            .demo_fleet()
            .filter(|_| demo_exe().is_some_and(|exe| exe.is_file()))
        else {
            return Response::error("cannot find daifuku.exe next to the daemon");
        };
        self.open_fleet(&fleet)
    }

    fn demo_fleet(&self) -> Option<daifuku_core::config::Fleet> {
        let exe = demo_exe()?;
        // The demo goes where the first fleet goes: its monitor, and the part
        // of it that fleet fills.
        let first = self.config.fleets.first();
        Some(daifuku_core::config::Fleet {
            name: DEMO.to_owned(),
            count: 6,
            monitor: first.map(|f| f.monitor.clone()).unwrap_or_default(),
            area: first.map(|f| f.area).unwrap_or_default(),
            // The drive root: nothing personal in the path if a prompt shows.
            directory: Some(std::path::PathBuf::from(r"C:\")),
            command: Some(demo_command(&exe)),
            // Administrator terminals when the daemon can open them, like the
            // fleets people run for real: a tiling window manager leaves those
            // alone, so the grid stays the grid.
            admin: self.elevated,
            no_profile: true,
            hotkey: None,
            ..daifuku_core::config::Fleet::default()
        })
    }

    /// A fleet's definition: from the config, or the built-in demo.
    fn definition(&self, name: &str) -> Option<daifuku_core::config::Fleet> {
        self.config.fleet(name).cloned().or_else(|| {
            if name.eq_ignore_ascii_case(DEMO) {
                self.demo_fleet()
            } else {
                None
            }
        })
    }

    fn snap(&mut self) -> Response {
        let mut snapped = 0;
        let definitions: Vec<_> = self
            .fleets
            .iter()
            .map(|r| self.definition(&r.name))
            .collect();
        for (record, definition) in self.fleets.iter_mut().zip(definitions) {
            if let Some(fleet) = definition
                && fleet::snap(&fleet, &self.config, record, true).is_ok()
            {
                snapped += record.windows().count();
            }
        }
        self.drop_closed_fleets();
        self.refresh_borders();
        Response::said(format!("snapped {}", fleet::terminals_count(snapped)))
    }

    fn close(&mut self, name: Option<&str>) -> Response {
        let index = match name {
            Some(n) => self
                .fleets
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(n)),
            None => self.config.fleets.first().and_then(|first| {
                self.fleets
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(&first.name))
            }),
        };
        // A name the config does not know is a mistake, as it is for `open`;
        // a fleet it knows that is not open is already closed.
        let Some(index) = index else {
            return match name {
                Some(n) if self.definition(n).is_none() => Response::error(format!(
                    "no fleet called `{n}` in {}",
                    self.config_path.display()
                )),
                None if self.config.fleets.is_empty() => {
                    Response::error("the config has no fleets")
                }
                _ => Response::said("that fleet is not open"),
            };
        };
        let name = self.fleets[index].name.clone();
        let (closed, open) = self.close_fleets(&[index]);
        Response::said(closed_message(Some(&name), closed, open))
    }

    fn close_all(&mut self) -> Response {
        if self.fleets.is_empty() {
            return Response::said("no fleet is open");
        }
        let every: Vec<usize> = (0..self.fleets.len()).collect();
        let (closed, open) = self.close_fleets(&every);
        Response::said(closed_message(None, closed, open))
    }

    /// Asks every terminal of the fleets at `indices` to close, waits for
    /// them together, and returns how many went and how many are still open.
    fn close_fleets(&mut self, indices: &[usize]) -> (usize, usize) {
        let mut asked: Vec<u64> = Vec::new();
        for &index in indices {
            let record = &mut self.fleets[index];
            // Only a window that is still one of our terminals: a handle can
            // have been reused by anything since.
            record.prune();
            asked.extend(record.windows().filter(|&w| window::close(w)));
        }
        // Windows Terminal asks before it closes a window with several
        // tabs. A terminal still open after that stays in its fleet, in its
        // own cell, and is counted as open rather than closed.
        let start = std::time::Instant::now();
        while asked.iter().any(|&w| window::exists(w)) && start.elapsed() < CLOSE_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let mut open = 0;
        for &index in indices {
            let record = &mut self.fleets[index];
            record.prune();
            open += record.windows().count();
        }
        let closed = asked.iter().filter(|&&w| !window::exists(w)).count();
        self.drop_closed_fleets();
        (closed, open)
    }

    /// The terminals whose agents need you, the one that has waited longest
    /// first.
    fn waiting(&self) -> Vec<u64> {
        self.agents
            .needs_you()
            .into_iter()
            .filter(|&w| window::exists(w))
            .collect()
    }

    fn next(&mut self) -> Response {
        let queue = self.waiting();
        let since = |w: u64| window_since(&self.agents, &self.shown_since, w);
        let Some(target) = next_target(&queue, window::foreground(), self.last_next, since) else {
            return Response::said("no agent is waiting");
        };
        if window::focus(target) {
            self.last_next = since(target).map(|t| (target, t));
            Response::said(format!("focused {}", window::title(target)))
        } else {
            Response::error("Windows refused to switch to that terminal")
        }
    }

    fn status(&self) -> Status {
        let agents = self
            .agents
            .windows()
            .into_iter()
            .map(|(w, state)| AgentWindow {
                window: w,
                title: window::title(w),
                state,
                for_seconds: window_since(&self.agents, &self.shown_since, w)
                    .map_or(0, |t| t.elapsed().as_secs()),
                label: self.label_of(w),
                fleet: self
                    .fleets
                    .iter()
                    .find(|f| f.slots.contains(&Some(w)))
                    .map(|f| f.name.clone()),
                session_id: shown_session(&self.agents, w, state),
            })
            .collect();
        let fleets = self
            .fleets
            .iter()
            .map(|f| FleetStatus {
                name: f.name.clone(),
                monitor: f.monitor.clone(),
                windows: f.windows().collect(),
            })
            .collect();
        let mut config = self.config_path.display().to_string();
        if let Some(e) = &self.config_error {
            let using = if self.on_defaults() {
                "the defaults"
            } else {
                "the last good one"
            };
            config.push_str(&format!(" (invalid, using {using}: {e})"));
        }
        let mut hotkeys = self.hotkeys.describe();
        hotkeys.extend(
            self.hotkeys
                .refused
                .iter()
                .map(|k| format!("{k}: refused, another program holds it")),
        );
        let peers = self
            .peers
            .iter()
            .map(|p| PeerStatus {
                window: p.window,
                label: p.spawn.label.clone(),
                directory: p.spawn.directory.display().to_string(),
                session_id: p.spawn.resume.clone(),
            })
            .collect();
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            config,
            elevated: self.elevated,
            hotkeys,
            fleets,
            agents,
            peers,
        }
    }

    fn window_events(&mut self) {
        let events = events::drain();
        if events.is_empty() {
            return;
        }
        let mut refresh = false;
        let mut restack = false;
        for e in events {
            match e {
                Event::Destroyed(w) => refresh |= self.window_destroyed(w),
                Event::Moved(w) | Event::Visibility(w) => {
                    refresh |= self.agents.window_state(w).is_some();
                }
                // Any window, not only an agent's: a terminal also comes up
                // when one of its own dialogs takes the foreground.
                Event::Foreground(_) => restack = true,
            }
        }
        if refresh {
            self.refresh_borders();
        }
        if restack && let Some(borders) = &self.borders {
            // A raised window keeps its rectangle and colour, so the pass
            // above sends nothing for it, yet its frame is now underneath it.
            if let Err(e) = borders.restack() {
                tracing::warn!(error = %e, "could not restack borders");
            }
        }
    }

    /// Watches agent process `pid` for the end of `session`. A process that
    /// cannot be opened, as one that ended already, is not watched; past
    /// [`MAX_SESSIONS`] processes, new ones are not either.
    fn watch_agent(&mut self, pid: u32, session: &str) {
        if let Some((_, sessions)) = self.agent_processes.get_mut(&pid) {
            if sessions.len() < MAX_SESSIONS {
                sessions.insert(session.to_owned());
            }
            return;
        }
        if self.agent_processes.len() >= MAX_SESSIONS {
            return;
        }
        if let Some(watch) = process::Watch::open(pid) {
            self.agent_processes
                .insert(pid, (watch, std::iter::once(session.to_owned()).collect()));
        }
    }

    /// Forgets the sessions of every agent whose process ended, and takes
    /// their borders down where no other agent in the window remains. Lets
    /// go of processes whose sessions all ended otherwise.
    fn reap_agents(&mut self) {
        let ended: Vec<u32> = self
            .agent_processes
            .iter()
            .filter(|(_, (watch, _))| watch.ended())
            .map(|(&pid, _)| pid)
            .collect();
        let mut forgot = false;
        for pid in ended {
            if let Some((_, sessions)) = self.agent_processes.remove(&pid) {
                for session in &sessions {
                    forgot |= self.agents.forget_session(session);
                }
                tracing::info!(pid, sessions = sessions.len(), "an agent's process ended");
            }
        }
        let agents = &self.agents;
        self.agent_processes
            .retain(|_, (_, sessions)| sessions.iter().any(|s| agents.knows(s)));
        if forgot {
            self.note_states();
            self.refresh_borders();
        }
    }

    fn sweep(&mut self) {
        self.reap_agents();
        // The config changed on disk: take it, as `daifuku reload` would.
        if stamp(&self.config_path) != self.config_stamp {
            tracing::info!("config file changed, reloading");
            let _ = self.control(&Request::Reload);
        }
        // A monitor came, went or changed (a plug, a resolution, a scale, a
        // display link renegotiating, a taskbar): an open fleet whose cells
        // moved goes back into the grid of the monitor it belongs on now.
        let now = daifuku_win::monitor::monitors();
        if now != self.monitors {
            tracing::info!(
                before = self.monitors.len(),
                after = now.len(),
                "monitors changed"
            );
            let before = std::mem::replace(&mut self.monitors, now);
            // A border's corners are worked out at the DPI of its screen, so
            // every border is handed over again, whether its window moved or
            // not.
            if let Some(b) = &self.borders {
                b.invalidate();
            }
            self.follow_monitors(&before);
            self.refresh_borders();
        }
        let (hc, anim) = (access::high_contrast(), access::animations());
        if (hc, anim) != (self.high_contrast, self.animations) {
            tracing::info!(
                high_contrast = hc,
                animations = anim,
                "Windows accessibility settings changed"
            );
            self.high_contrast = hc;
            self.animations = anim;
            self.refresh_borders();
        } else if self.high_contrast {
            // Another contrast theme, or the colours of this one edited,
            // with high contrast on all along: only the colours change, and
            // only a repaint shows them. One that changes nothing sends
            // nothing to the borders.
            self.paint_borders();
        }
        let gone = away();
        if gone != self.away {
            tracing::info!(away = gone, "the desktop was left or came back");
            self.away = gone;
            self.refresh_borders();
        }
        let dead: Vec<u64> = self
            .agents
            .windows()
            .into_keys()
            .filter(|&w| !window::exists(w))
            .collect();
        for w in &dead {
            self.agents.forget_window(*w);
        }
        if !dead.is_empty() {
            self.note_states();
        }
        for f in &mut self.fleets {
            f.prune();
        }
        self.drop_closed_fleets();
        if !dead.is_empty() {
            self.refresh_borders();
        }
        self.save(false);
    }

    /// Writes the state to [`Daemon::state_file`] when it changed since the
    /// last save, when that save is a minute old, or always when `now`.
    fn save(&mut self, now: bool) {
        let Some(file) = &self.state_file else {
            return;
        };
        let ticks = clock::ticks();
        let mut saved = self.to_saved(ticks);
        let shown: Vec<_> = saved
            .shown
            .iter()
            .map(|s| (&s.session, s.window, s.state))
            .collect();
        let Ok(key) = serde_json::to_string(&(&saved.agents, &saved.fleets, shown)) else {
            return;
        };
        let (last, at) = &self.last_saved;
        if !now && key == *last && ticks.saturating_sub(*at) < RESAVE_TICKS {
            return;
        }
        saved.boot = saved::boot(wall_seconds(), ticks);
        saved.at = ticks;
        let written = serde_json::to_string(&saved)
            .map_err(std::io::Error::other)
            .and_then(|text| replace_file(file, &text));
        match written {
            Ok(()) => self.last_saved = (key, ticks),
            Err(e) => {
                tracing::warn!(path = %file.display(), error = %e, "could not save the state")
            }
        }
    }

    /// The state as it is saved at interrupt time `ticks`, its own time
    /// still unset.
    fn to_saved(&self, ticks: u64) -> Saved {
        let now = std::time::Instant::now();
        let mut shown: Vec<SavedShown> = self
            .shown_since
            .iter()
            .map(|((session, window), &(state, t))| SavedShown {
                session: session.clone(),
                window: *window,
                state,
                since: ticks.saturating_sub(
                    u64::try_from(now.duration_since(t).as_nanos() / 100).unwrap_or(u64::MAX),
                ),
            })
            .collect();
        // In a set order, so a state that did not change saves the same.
        shown.sort_by(|a, b| (&a.session, a.window).cmp(&(&b.session, b.window)));
        Saved {
            boot: 0,
            at: 0,
            agents: self.agents.clone(),
            fleets: self
                .fleets
                .iter()
                .map(|f| SavedFleet {
                    name: f.name.clone(),
                    monitor: f.monitor.clone(),
                    slots: f.slots.clone(),
                })
                .collect(),
            shown,
        }
    }

    /// Takes back the state the daemon before this one saved, when it is
    /// from this start of Windows and fresh. Every window in it is checked
    /// again: an agent's must still be a window a hook could name, a fleet's
    /// still a terminal.
    fn restore(&mut self) {
        let Some(own) = &self.state_file else {
            return;
        };
        let file = state_to_read(own, self.legacy_state_file.as_deref());
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(path = %file.display(), error = %e, "could not read the saved state");
                return;
            }
        };
        let saved = match serde_json::from_str::<Saved>(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "the saved state does not read, starting clean");
                return;
            }
        };
        let ticks = clock::ticks();
        if !saved.usable(saved::boot(wall_seconds(), ticks), ticks) {
            tracing::info!("the saved state is from another start of Windows or too old");
            return;
        }
        self.take_back(saved, ticks, |w| {
            daifuku_win::console::visible_window(w) == Some(w)
        });
        tracing::info!(
            sessions = self.agents.len(),
            fleets = self.fleets.len(),
            "took back the saved state"
        );
    }

    /// Puts `saved` in place at interrupt time `ticks`, keeping only the
    /// agents whose window passes `alive`, and only the fleet terminals that
    /// are still terminals.
    fn take_back(&mut self, saved: Saved, ticks: u64, alive: impl Fn(u64) -> bool) {
        self.agents = saved.agents;
        let gone: Vec<u64> = self
            .agents
            .windows()
            .into_keys()
            .filter(|&w| !alive(w))
            .collect();
        for w in gone {
            self.agents.forget_window(w);
        }
        self.fleets = saved
            .fleets
            .into_iter()
            .map(|f| {
                let mut record = OpenFleet::new(&f.name);
                record.monitor = f.monitor;
                record.slots = f.slots;
                record.prune();
                record
            })
            .collect();
        self.drop_closed_fleets();
        let now = std::time::Instant::now();
        self.shown_since = saved
            .shown
            .into_iter()
            .map(|s| {
                let ago = std::time::Duration::from_nanos(
                    ticks.saturating_sub(s.since).saturating_mul(100),
                );
                (
                    (s.session, s.window),
                    (s.state, now.checked_sub(ago).unwrap_or(now)),
                )
            })
            .collect();
        // Drops what was forgotten above and times anything missing from now.
        self.note_states();
    }

    /// Snaps back only the open fleets whose cells moved when the monitors
    /// changed from `before` to what they are now. A terminal
    /// someone minimised or maximised stays so: only a snap they ask for
    /// restores it.
    fn follow_monitors(&mut self, before: &[daifuku_core::monitor::MonitorInfo]) {
        let cursor = daifuku_win::monitor::cursor();
        let definitions: Vec<_> = self
            .fleets
            .iter()
            .map(|r| self.definition(&r.name))
            .collect();
        for (record, definition) in self.fleets.iter_mut().zip(definitions) {
            if let Some(fleet) = definition
                && fleet::moved(&fleet, &self.config, record, before, &self.monitors, cursor)
            {
                tracing::info!(fleet = %record.name, "monitors changed, snapping the fleet");
                let _ = fleet::snap(&fleet, &self.config, record, false);
            }
        }
        self.drop_closed_fleets();
    }

    /// Lets go of every fleet whose terminals are all closed, so it is no
    /// longer listed as open. Its record holds nothing worth keeping: a fleet
    /// opened again starts from its config, as a new one does.
    fn drop_closed_fleets(&mut self) {
        self.fleets.retain(|f| f.windows().next().is_some());
        let before = self.peers.len();
        self.peers
            .retain(|p| window::exists(p.window) && window::class(p.window) == WINDOW_CLASS);
        if self.peers.len() != before {
            self.save_peers();
        }
        // A handle Windows hands to a new window must not bring a label.
        self.labels.retain(|&w, _| window::exists(w));
    }

    /// Lets go of window `w`, which was destroyed: its agents, its cell in a
    /// fleet, its peer, which leaves the peers file, and its label. Says
    /// whether an agent was forgotten.
    fn window_destroyed(&mut self, w: u64) -> bool {
        let forgot = self.agents.forget_window(w);
        if forgot {
            self.note_states();
        }
        for f in &mut self.fleets {
            f.forget(w);
        }
        let peers = self.peers.len();
        self.peers.retain(|p| p.window != w);
        if self.peers.len() != peers {
            self.save_peers();
        }
        self.labels.remove(&w);
        self.drop_closed_fleets();
        forgot
    }

    /// What to call terminal `w`: the label `daifuku label` or `spawn` gave
    /// it, else its fleet's name and its number in the fleet.
    fn label_of(&self, w: u64) -> Option<String> {
        if let Some(label) = self.labels.get(&w) {
            return Some(label.clone());
        }
        if let Some(p) = self.peers.iter().find(|p| p.window == w) {
            return p.spawn.label.clone();
        }
        self.fleets.iter().find_map(|f| {
            f.slots
                .iter()
                .position(|s| *s == Some(w))
                .map(|i| format!("{} {}", f.name, i + 1))
        })
    }

    /// Gives terminal `w` a label, or takes it away with `None`. A peer's
    /// label is kept with it across a restart.
    fn label(&mut self, w: u64, label: Option<&str>) -> Response {
        if let Some(text) = label
            && let Some(problem) = (Spawn {
                label: Some(text.to_owned()),
                ..Spawn::default()
            })
            .problem()
        {
            return Response::error(problem);
        }
        if !window::exists(w) {
            return Response::error(format!("there is no window {w:#010x}"));
        }
        if let Some(p) = self.peers.iter_mut().find(|p| p.window == w) {
            p.spawn.label = label.map(str::to_owned);
            self.save_peers();
        } else if let Some(text) = label {
            self.labels.insert(w, text.to_owned());
        } else {
            self.labels.remove(&w);
        }
        Response::ok()
    }

    /// Notes `session` as the one peer window `w` runs now, so the peer
    /// resumes it after a restart of Windows.
    ///
    /// The hook pipe is open to every process of the user, and `restore`
    /// runs the session in an administrator terminal. So a session is kept
    /// only when it can go into a command line, and only from `agent`, the
    /// process the hook ran under, when that process is in the peer's own
    /// terminal: its console leads to `w`. A process of the user outside the
    /// peer cannot be under the peer's agent, and one that is not elevated
    /// cannot reach an administrator peer's console.
    fn note_peer_session(&mut self, w: u64, session: &str, agent: Option<u32>) {
        if !peer::session_id_ok(session) {
            return;
        }
        let Some(i) = self.peers.iter().position(|p| p.window == w) else {
            return;
        };
        if self.peers[i].spawn.resume.as_deref() == Some(session) {
            return;
        }
        if !agent.is_some_and(|a| in_terminal(a, w)) {
            tracing::warn!(
                window = format!("{w:#x}"),
                "a hook outside the peer named its session; kept the old one"
            );
            return;
        }
        self.peers[i].spawn.resume = Some(session.to_owned());
        self.save_peers();
    }

    /// Writes the peers, open and dormant, to [`Daemon::peers_file`]. Not
    /// while the session ends: its terminals close then, and the file must
    /// keep them for `daifuku restore`.
    fn save_peers(&self) {
        let Some(file) = &self.peers_file else {
            return;
        };
        if process::session_ending() {
            return;
        }
        let boot = saved::boot(wall_seconds(), clock::ticks());
        let kept = KeptFile {
            boot,
            dormant_boot: self.dormant_boot,
            peers: self
                .peers
                .iter()
                .map(|p| Kept {
                    spawn: p.spawn.clone(),
                    window: p.window,
                    boot,
                })
                .chain(self.dormant.iter().map(|spawn| Kept {
                    spawn: spawn.clone(),
                    window: 0,
                    boot: self.dormant_boot,
                }))
                .collect(),
        };
        let written = serde_json::to_string_pretty(&kept)
            .map_err(std::io::Error::other)
            .and_then(|text| replace_file(file, &text));
        if let Err(e) = written {
            tracing::warn!(path = %file.display(), error = %e, "could not save the peers");
        }
    }

    /// Reads [`Daemon::peers_file`]: a peer whose terminal is still open,
    /// as after an install, is open again; one that was open when Windows
    /// last stopped is dormant, when it has a session to resume. Peers from
    /// any start of Windows before that are dropped, as is one closed in this
    /// start of Windows while no daemon ran. Written back at once, so the
    /// file says which start of Windows its dormant peers are from.
    fn load_peers(&mut self) {
        let Some(file) = &self.peers_file else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        let boot = saved::boot(wall_seconds(), clock::ticks());
        let (open, dormant, dormant_boot) = sort_kept(peer::read_kept(&text), boot, |w| {
            window::exists(w) && window::class(w) == WINDOW_CLASS
        });
        self.peers = open
            .into_iter()
            .map(|(window, spawn)| Peer { window, spawn })
            .collect();
        self.dormant = dormant;
        self.dormant_boot = dormant_boot;
        self.save_peers();
        tracing::info!(
            open = self.peers.len(),
            dormant = self.dormant.len(),
            "read the peers"
        );
    }

    /// Opens the dormant peers again, each resuming its session in its own
    /// folder; or with `dry_run` only lists them, and with `forget` drops
    /// them.
    fn restore_peers(&mut self, dry_run: bool, forget: bool) -> Response {
        let listed = |s: &Spawn, window: Option<u64>, error: Option<String>| RestoredPeer {
            label: s.label.clone(),
            directory: s.directory.display().to_string(),
            session_id: s.resume.clone().unwrap_or_default(),
            window,
            error,
        };
        if dry_run || forget {
            let peers = self.dormant.iter().map(|s| listed(s, None, None)).collect();
            if forget && !dry_run {
                self.dormant.clear();
                self.save_peers();
            }
            return Response::Restored { peers };
        }
        // Each stays dormant until it is open, so a daemon that stops part
        // way loses none of the rest.
        let mut peers = Vec::new();
        for spawn in self.dormant.clone() {
            if let Some(running) = self.running(&spawn) {
                peers.push(listed(&spawn, None, Some(running)));
                self.dormant.retain(|d| *d != spawn);
                continue;
            }
            match self.spawn(&spawn) {
                Response::Spawned { window, .. } => {
                    peers.push(listed(&spawn, Some(window), None));
                    if let Some(i) = self.dormant.iter().position(|d| *d == spawn) {
                        self.dormant.remove(i);
                    }
                }
                Response::Error { message } => peers.push(listed(&spawn, None, Some(message))),
                _ => {}
            }
        }
        self.save_peers();
        Response::Restored { peers }
    }

    /// Why a dormant peer must not be opened again, when its session runs
    /// already: in an open peer, or anywhere an agent reports it from. Two
    /// agents on one session would write over each other.
    fn running(&self, spawn: &Spawn) -> Option<String> {
        let id = spawn.resume.as_deref()?;
        let open = self
            .peers
            .iter()
            .any(|p| p.spawn.resume.as_deref() == Some(id));
        (open || self.agents.knows(id)).then(|| format!("session {id} runs already"))
    }

    /// Opens one peer as `spawn` asks, from the fleet it names or the first
    /// one, or from the defaults when the config has no fleet.
    fn spawn(&mut self, spawn: &Spawn) -> Response {
        if let Some(problem) = spawn.problem() {
            return Response::error(problem);
        }
        let base = match &spawn.fleet {
            Some(name) => match self.config.fleet(name).cloned() {
                Some(f) => f,
                None => {
                    return Response::error(format!(
                        "no fleet called `{name}` in {}",
                        self.config_path.display()
                    ));
                }
            },
            None => self.config.fleets.first().cloned().unwrap_or_default(),
        };
        // Looked for as whoever the terminal runs as: a drive letter of the
        // user's, as `subst` makes, does not exist for an elevated daemon.
        if !spawn.directory.is_absolute()
            || !fleet::folder_exists(&spawn.directory, fleet::as_user(&base, self.elevated))
        {
            return Response::error(format!("there is no folder {}", spawn.directory.display()));
        }
        self.drop_closed_fleets();
        let taken: Vec<u64> = self
            .fleets
            .iter()
            .flat_map(OpenFleet::windows)
            .chain(self.peers.iter().map(|p| p.window))
            .collect();
        match fleet::spawn(&base, &self.config, spawn, self.elevated, &taken) {
            Ok(window) => {
                tracing::info!(window = format!("{window:#x}"), label = ?spawn.label, "spawned a peer");
                self.peers.push(Peer {
                    window,
                    spawn: spawn.clone(),
                });
                self.save_peers();
                self.refresh_borders();
                Response::Spawned {
                    window,
                    label: spawn.label.clone(),
                }
            }
            Err(e) => Response::error(format!("{e:#}")),
        }
    }

    /// Sends the borders their end state: one frame per agent window that is
    /// on screen, in its state's colour and at its state's width. A maximised
    /// window gets none: its frame is the whole screen, and a border outside
    /// it would land on the taskbar and the next monitor. Starts or
    /// stops the pulse to match: it runs only while a waiting agent's window
    /// has a frame, so not for one that is minimised or cloaked away. Hooks
    /// the window events borders follow their windows by while there is an
    /// agent to draw around, and unhooks them once there is none.
    fn refresh_borders(&mut self) {
        self.hooks.follow(
            self.borders.is_some() && self.config.border.enabled && !self.agents.is_empty(),
        );
        self.framed = self
            .agents
            .windows()
            .into_iter()
            .filter(|&(w, _)| {
                window::is_shown(w) && !window::is_minimised(w) && !window::is_maximised(w)
            })
            .filter_map(|(w, state)| window::frame(w).map(|rect| (w, state, rect)))
            .collect();
        self.update_pulse(
            self.framed
                .iter()
                .any(|&(_, state, _)| state == AgentState::Waiting),
        );
        self.paint_borders();
    }

    /// Hands the frames the last [`Daemon::refresh_borders`] measured to the
    /// border thread, a waiting one at the pulse's brightness of the moment.
    /// This alone is a pulse tick: only the waiting colours change, and a
    /// window that moved, changed or went away has already brought a full
    /// pass through its window event.
    fn paint_borders(&self) {
        let Some(borders) = &self.borders else { return };
        let border = &self.config.border;
        let colours = if self.high_contrast {
            access::high_contrast_colours()
        } else {
            border.colours()
        };
        let breath = self
            .pulse_timer
            .map(|_| breath(self.started.elapsed().as_secs_f64()));
        let specs: Vec<BorderSpec> = self
            .framed
            .iter()
            .map(|&(w, state, rect)| {
                let mut colour = colours.of(state);
                if state == AgentState::Waiting
                    && let Some(k) = breath
                {
                    colour = dim(colour, k);
                }
                BorderSpec::new(WindowHandle::from_raw(w), rect, colour).with_width(width_for(
                    border,
                    state,
                    self.high_contrast,
                ))
            })
            .collect();
        if let Err(e) = borders.update(None, specs) {
            tracing::warn!(error = %e, "could not update borders");
        }
    }

    /// Starts the pulse timer when a waiting agent has a frame on screen and
    /// the pulse is wanted and allowed, and stops it otherwise. A high
    /// contrast theme holds it still: dimming the theme's own colours takes
    /// away the contrast it is for. It also rests while nobody is at the
    /// desktop, where it would repaint for hours with no one to see it.
    fn update_pulse(&mut self, waiting: bool) {
        let b = &self.config.border;
        let wanted =
            b.enabled && b.pulse && self.animations && !self.high_contrast && !self.away && waiting;
        match (wanted, self.pulse_timer) {
            (true, None) => {
                // SAFETY: a thread timer, killed below or at exit.
                let id = unsafe { SetTimer(None, 0, PULSE_MS, None) };
                self.pulse_timer = (id != 0).then_some(id);
            }
            (false, Some(id)) => {
                // SAFETY: the timer this thread set.
                unsafe {
                    let _ = KillTimer(None, id);
                }
                self.pulse_timer = None;
            }
            _ => {}
        }
    }
}

/// Whether nobody is at the desktop, as Windows tells a program that wants
/// to show a notification: the session is locked, a screen saver is up, or
/// another account was switched to.
fn away() -> bool {
    // SAFETY: no arguments; only reads the state of the session.
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|state| state == QUNS_NOT_PRESENT)
}

/// How bright a waiting border is at `t` seconds: a slow breath between 70 %
/// and full, never off, so the state stays readable at every moment. Dimmer
/// than 70 % takes the colour-blind palette's orange below 3:1 against a dark
/// background.
fn breath(t: f64) -> f64 {
    let phase = (t / PULSE_PERIOD) * std::f64::consts::TAU;
    0.70 + 0.30 * (0.5 + 0.5 * phase.cos())
}

/// The width a state's border is drawn at. With a high contrast theme on,
/// every state has its own width even when `state_widths` is off: the theme
/// picks the colours, and two of them can look alike.
fn width_for(border: &Border, state: AgentState, high_contrast: bool) -> i32 {
    Border {
        state_widths: border.state_widths || high_contrast,
        ..*border
    }
    .width_for(state)
}

/// The fleets opening `fleet` opens, in order: the ones its `with` names
/// first, then `fleet` itself, so it ends up in front and its first terminal
/// has the focus. The others' own `with` is not followed, and a name that is
/// not in the config is skipped.
fn opening_order(
    config: &Config,
    fleet: daifuku_core::config::Fleet,
) -> Vec<daifuku_core::config::Fleet> {
    let mut order: Vec<_> = fleet
        .with
        .iter()
        .filter_map(|other| config.fleet(other).cloned())
        .collect();
    order.push(fleet);
    order
}

/// The `daifuku.exe` in the folder this daemon runs from.
fn demo_exe() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("daifuku.exe"))
}

/// The command a demo terminal runs, `{n}` standing for its number.
fn demo_command(exe: &std::path::Path) -> String {
    format!(
        "& {} demo-agent {{n}}",
        single_quoted(&exe.display().to_string())
    )
}

/// `text` as a PowerShell string in single quotes, which takes everything as
/// written but the quote itself, doubled. PowerShell also reads the
/// typographic single quotes as one, so a folder named O’Neill needs them
/// doubled too.
fn single_quoted(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// What `close` says about fleet `name`, or about every fleet without one:
/// how many terminals closed, and how many are still `open`.
fn closed_message(name: Option<&str>, closed: usize, open: usize) -> String {
    let of = name.map(|n| format!(" of {n}")).unwrap_or_default();
    if open == 0 {
        return format!("closed {}{of}", fleet::terminals_count(closed));
    }
    format!(
        "closed {closed} of {}{of}; Windows Terminal kept {} open, perhaps to ask about closing tabs",
        fleet::terminals_count(closed + open),
        fleet::terminals_count(open)
    )
}

/// A colour at `k` of its brightness.
fn dim(c: daifuku_core::config::Colour, k: f64) -> daifuku_core::config::Colour {
    let f = |v: u8| {
        // In range: v is at most 255 and k at most 1.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let out = (f64::from(v) * k).round().clamp(0.0, 255.0) as u8;
        out
    };
    daifuku_core::config::Colour::new(f(c.r), f(c.g), f(c.b))
}

/// The wall clock, in seconds since 1970.
fn wall_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The agent's process a hook named, when the pipe says the hook ran under
/// it: one of the client's ancestors, not the client itself.
fn trusted_agent(named: Option<u32>, sender: &[u32]) -> Option<u32> {
    named.filter(|p| sender.iter().skip(1).any(|s| s == p))
}

/// How long the daemon waits for `daifuku console-window`.
const CONSOLE_HELPER_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether process `pid` runs in the terminal that is window `w`: its
/// console, or one of its ancestors', leads there.
///
/// Asked of `daifuku console-window`, next to the daemon, never found out
/// here: finding out means attaching to the agent's console, and a process
/// attached to a console that closes is ended with it. The helper may end;
/// the daemon must not.
fn in_terminal(pid: u32, w: u64) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Some(exe) = std::env::current_exe()
        .ok()
        .map(|d| d.with_file_name("daifuku.exe"))
        .filter(|e| e.is_file())
    else {
        return false;
    };
    let Ok(mut helper) = std::process::Command::new(exe)
        .args(["console-window", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    else {
        return false;
    };
    let start = std::time::Instant::now();
    loop {
        match helper.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < CONSOLE_HELPER_WAIT => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                let _ = helper.kill();
                let _ = helper.wait();
                return false;
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = helper.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
    }
    out.trim().parse::<u64>().ok() == Some(w)
}

/// The session whose state window `w` shows as `state`: of the sessions in
/// it, the first in that state.
fn shown_session(agents: &Agents<u64>, w: u64, state: AgentState) -> Option<String> {
    agents
        .sessions()
        .find(|&(_, window, s)| window == w && s == state)
        .map(|(id, _, _)| id.to_owned())
}

/// Sorts the peers read from `file` into the ones still open, by their
/// window, and the dormant ones, with the start of Windows those are from.
///
/// A peer is open when it was saved in this start of Windows, by the clock
/// `boot` worked out now, and its window passes `alive`. Dormant are the
/// peers of one start of Windows only: the one the file was written in when
/// that was an earlier one, as after a restart; else, as after a daemon
/// restart, the one its dormant peers were already from. Of those, only the
/// ones with a session to resume. Everything else is dropped: older peers,
/// and peers of this start of Windows whose terminal closed.
fn sort_kept(
    file: KeptFile,
    boot: u64,
    alive: impl Fn(u64) -> bool,
) -> (Vec<(u64, Spawn)>, Vec<Spawn>, u64) {
    let now = |b: u64| b.abs_diff(boot) <= 10;
    let from = if now(file.boot) {
        file.dormant_boot
    } else {
        file.boot
    };
    let mut open = Vec::new();
    let mut dormant = Vec::new();
    for k in file.peers {
        if k.window != 0 && k.same_boot(boot) && alive(k.window) {
            open.push((k.window, k.spawn));
        } else if from != 0 && !now(k.boot) && k.same_boot(from) && k.resumable() {
            dormant.push(k.spawn);
        }
    }
    (open, dormant, from)
}

/// Where one user's daemon keeps what it saves.
#[derive(Debug, PartialEq, Eq)]
struct SavedFiles {
    /// The fleets and agent states, for a restart in this start of Windows.
    state: PathBuf,
    /// The peers, for `daifuku restore`.
    peers: PathBuf,
    /// The state file versions up to 0.1.3 shared between users.
    legacy_state: Option<PathBuf>,
}

impl SavedFiles {
    /// The files in `dir` for the user `sid`: one pair per user, as every
    /// user's daemon writes into the same folder. Without a SID, the shared
    /// names, as there is nothing to tell users apart by.
    fn in_dir(dir: &std::path::Path, sid: Option<&str>) -> Self {
        let name = |what: &str| match sid {
            Some(sid) => format!("{what}-{sid}.json"),
            None => format!("{what}.json"),
        };
        Self {
            state: dir.join(name("state")),
            peers: dir.join(name("peers")),
            legacy_state: sid.map(|_| dir.join("state.json")),
        }
    }
}

/// The state file to read: the user's own, or the one earlier versions
/// shared while the user's own does not exist yet.
fn state_to_read<'a>(
    own: &'a std::path::Path,
    legacy: Option<&'a std::path::Path>,
) -> &'a std::path::Path {
    match legacy {
        Some(legacy) if !own.exists() => legacy,
        _ => own,
    }
}

/// Writes `text` next to `path` and renames it over `path`, so a daemon that
/// stops half way leaves the last whole save, never one cut short.
fn replace_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    let new = path.with_file_name(name);
    // On the disk before the rename, so a power cut leaves the old file or
    // the new one, never an empty one.
    let mut f = std::fs::File::create(&new)?;
    std::io::Write::write_all(&mut f, text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&new, path)
}

/// When a file last changed, `None` when it does not exist.
fn stamp(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Notes that reading the config file saved at `stamp` failed. An editor can
/// hold the file for a moment while it saves: forget the stamp so the next
/// sweep reads it once more. Only once per save: a file that stays unreadable
/// is not read on every sweep, and the next save is tried afresh.
fn retry_once(
    failed: &mut Option<std::time::SystemTime>,
    stamp: &mut Option<std::time::SystemTime>,
) {
    if *failed != *stamp {
        *failed = *stamp;
        *stamp = None;
    }
}

/// Since when each session, in the window it runs in, has shown its state.
type Shown = std::collections::HashMap<(String, u64), (AgentState, std::time::Instant)>;

/// Since when window `w` has shown its state: since the earliest of its
/// sessions that shows that state began to. A window with two tabs waiting
/// has waited since the first of them still waiting began, not since a tab
/// that has been answered since.
fn window_since(agents: &Agents<u64>, shown: &Shown, w: u64) -> Option<std::time::Instant> {
    let state = agents.window_state(w)?;
    agents
        .sessions()
        .filter(|&(_, sw, s)| sw == w && s == state)
        .filter_map(|(id, _, _)| shown.get(&(id.to_owned(), w)).map(|&(_, t)| t))
        .min()
}

/// Keeps `shown` in step with every state as of `now`: one that changed is
/// shown since `now`, one that did not keeps its time, and one that is gone
/// is dropped.
fn note_shown<K: Ord + Eq + std::hash::Hash>(
    shown: &mut std::collections::HashMap<K, (AgentState, std::time::Instant)>,
    states: std::collections::BTreeMap<K, AgentState>,
    now: std::time::Instant,
) {
    shown.retain(|w, _| states.contains_key(w));
    for (w, state) in states {
        if shown.get(&w).map(|&(s, _)| s) != Some(state) {
            shown.insert(w, (state, now));
        }
    }
}

/// The terminal `next` brings up from `queue`, the agents that need you in
/// order; `None` when it is empty.
///
/// Pressed again from the terminal it brought up, while that agent still
/// waits as it did then, it moves on to the next one, wrapping at the end:
/// three waiting agents are three presses. From anywhere else it starts at
/// the front of the queue. A terminal in front for another reason, such as
/// the last one a fleet opened, or the one just approved whose agent has
/// since failed or asks again, is no place in the queue: moving on from it
/// skipped the agents that had waited longest. Measured on a demo, where it
/// landed on a failed agent that Enter cannot approve.
///
/// `front` is the window in front now, `last` the one `next` brought up with
/// the time its agent had shown its state then, and `since` gives that time
/// for a window now.
fn next_target<T: PartialEq + Copy>(
    queue: &[u64],
    front: u64,
    last: Option<(u64, T)>,
    since: impl Fn(u64) -> Option<T>,
) -> Option<u64> {
    let first = *queue.first()?;
    let from = last
        .filter(|&(w, t)| w == front && since(w) == Some(t))
        .and_then(|(w, _)| queue.iter().position(|&q| q == w));
    Some(match from {
        Some(i) => queue[(i + 1) % queue.len()],
        None => first,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A daemon on the defaults, whose config file does not exist, with
    /// nothing hooked, registered or drawn.
    fn daemon() -> Daemon {
        Daemon::new(
            PathBuf::from(r"C:\no\such\folder\daifuku.json"),
            false,
            events::Hooks::none(),
        )
    }

    #[test]
    fn a_fleet_whose_terminals_are_all_closed_is_no_longer_open() {
        let mut d = daemon();
        let mut record = OpenFleet::new("agents");
        record.slots = vec![None, None];
        d.fleets.push(record);
        d.sweep();
        assert!(d.status().fleets.is_empty());
    }

    #[test]
    fn a_peer_in_a_folder_that_is_not_there_is_refused_before_anything_starts() {
        let missing = std::env::current_dir().unwrap().join("no-such-folder-here");
        for (elevated, admin) in [(false, false), (true, false), (true, true)] {
            let mut d = daemon();
            d.elevated = elevated;
            d.config.fleets[0].admin = admin;
            for directory in [missing.clone(), PathBuf::from("relative")] {
                let said = d.spawn(&Spawn {
                    directory: directory.clone(),
                    ..Spawn::default()
                });
                assert_eq!(
                    said,
                    Response::error(format!("there is no folder {}", directory.display())),
                    "elevated {elevated}, admin {admin}"
                );
            }
            assert!(d.peers.is_empty());
        }
    }

    #[test]
    fn closing_a_fleet_the_config_does_not_know_is_an_error() {
        let mut d = daemon();
        assert!(matches!(
            d.close(Some("agnets")),
            Response::Error { message } if message.contains("no fleet called `agnets`")
        ));
        // A known fleet that is not open is closed already, demo included.
        for name in ["agents", "Demo"] {
            assert_eq!(
                d.close(Some(name)),
                Response::said("that fleet is not open"),
                "{name}"
            );
        }
        assert_eq!(d.close(None), Response::said("that fleet is not open"));
        d.config.fleets.clear();
        assert_eq!(d.close(None), Response::error("the config has no fleets"));
    }

    #[test]
    fn an_invalid_config_says_whether_the_defaults_are_in_use() {
        let dir = std::env::temp_dir().join(format!("daifukud-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daifuku.json");
        std::fs::write(&path, "{ not json").unwrap();
        let mut d = Daemon::new(path.clone(), false, events::Hooks::none());
        d.load_config();
        let started_invalid = d.status().config;
        std::fs::write(&path, r#"{"sound": true}"#).unwrap();
        d.load_config();
        std::fs::write(&path, "{ not json").unwrap();
        d.load_config();
        let broken_later = d.status().config;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            started_invalid.contains("(invalid, using the defaults: "),
            "{started_invalid}"
        );
        assert!(
            broken_later.contains("(invalid, using the last good one: "),
            "{broken_later}"
        );
    }

    #[test]
    fn an_agent_whose_process_ended_is_forgotten_and_the_others_stay() {
        use daifuku_core::state::HookEvent;
        let mut d = daemon();
        let event = |session: &str| HookEvent {
            session_id: session.into(),
            hook_event_name: "PermissionRequest".into(),
            ..HookEvent::default()
        };
        // Two agents in one window, as two tabs: one process ends, one runs.
        let mut ends = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "exit"])
            .spawn()
            .unwrap();
        let mut runs = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        d.agents.apply(5, &event("gone"));
        d.agents.apply(5, &event("here"));
        d.watch_agent(ends.id(), "gone");
        d.watch_agent(runs.id(), "here");
        ends.wait().unwrap();
        d.reap_agents();
        assert_eq!(d.agents.session_state("gone"), None);
        assert_eq!(d.agents.session_state("here"), Some(AgentState::Waiting));
        assert_eq!(d.agents.window_state(5), Some(AgentState::Waiting));
        assert_eq!(d.agent_processes.len(), 1);
        let _ = runs.kill();
        let _ = runs.wait();
        d.reap_agents();
        assert_eq!(
            d.agents.window_state(5),
            None,
            "no agent left in the window"
        );
        assert!(d.agent_processes.is_empty());
    }

    #[test]
    fn a_terminal_is_called_by_its_label_else_by_its_fleet_and_number() {
        let mut d = daemon();
        let mut record = OpenFleet::new("crew");
        record.slots = vec![Some(1), None, Some(3)];
        d.fleets.push(record);
        d.peers.push(Peer {
            window: 7,
            spawn: Spawn {
                label: Some("site".into()),
                ..Spawn::default()
            },
        });
        assert_eq!(d.label_of(1).as_deref(), Some("crew 1"));
        assert_eq!(d.label_of(3).as_deref(), Some("crew 3"));
        assert_eq!(d.label_of(7).as_deref(), Some("site"));
        assert_eq!(d.label_of(9), None);
        d.labels.insert(3, "lead".into());
        assert_eq!(d.label_of(3).as_deref(), Some("lead"));
    }

    #[test]
    fn a_label_with_a_line_break_or_for_no_window_is_refused() {
        let mut d = daemon();
        assert!(matches!(
            d.label(1, Some("a\nb")),
            Response::Error { message } if message.contains("label")
        ));
        assert!(matches!(
            d.label(0, Some("site")),
            Response::Error { message } if message.contains("no window")
        ));
    }

    #[test]
    fn the_session_shown_is_the_one_in_the_window_s_state() {
        use daifuku_core::state::HookEvent;
        let mut agents = Agents::new();
        let event = |session: &str, name: &str| HookEvent {
            session_id: session.into(),
            hook_event_name: name.into(),
            ..HookEvent::default()
        };
        agents.apply(5, &event("a", "UserPromptSubmit"));
        agents.apply(5, &event("b", "Stop"));
        let state = agents.window_state(5).unwrap();
        let shown = shown_session(&agents, 5, state).unwrap();
        assert_eq!(agents.session_state(&shown), Some(state));
        assert_eq!(shown_session(&agents, 6, state), None);
    }

    fn kept(resume: Option<&str>, window: u64, boot: u64) -> Kept {
        Kept {
            spawn: Spawn {
                directory: format!(r"C:\src\{window}").into(),
                resume: resume.map(Into::into),
                ..Spawn::default()
            },
            window,
            boot,
        }
    }

    fn sessions(spawns: &[Spawn]) -> Vec<&str> {
        spawns
            .iter()
            .map(|s| s.resume.as_deref().unwrap())
            .collect()
    }

    #[test]
    fn after_a_restart_only_the_peers_open_when_windows_stopped_come_back() {
        // Written in the start of Windows at 500, before this one at 1000.
        let file = KeptFile {
            boot: 500,
            dormant_boot: 300,
            peers: vec![
                // Open when Windows stopped, with and without a session.
                kept(Some("a"), 2, 500),
                kept(None, 3, 500),
                // Dormant since the start before that and never restored.
                kept(Some("old"), 0, 300),
            ],
        };
        // A window of the same number exists now, but in another start.
        let (open, dormant, from) = sort_kept(file, 1_000, |w| w == 2);
        assert!(open.is_empty());
        assert_eq!(sessions(&dormant), ["a"]);
        assert_eq!(from, 500);
    }

    #[test]
    fn after_a_daemon_restart_open_peers_stay_open_and_dormant_ones_dormant() {
        // Written earlier in this start of Windows, as before an install.
        let file = KeptFile {
            boot: 1_000,
            dormant_boot: 500,
            peers: vec![
                kept(Some("a"), 1, 1_000),
                kept(None, 2, 1_000),
                // Closed while no daemon ran: gone, not dormant.
                kept(Some("closed"), 4, 1_000),
                kept(Some("b"), 0, 500),
                kept(Some("older"), 0, 300),
            ],
        };
        let (open, dormant, from) = sort_kept(file, 1_005, |w| w == 1 || w == 2);
        let windows: Vec<_> = open.iter().map(|(w, _)| *w).collect();
        assert_eq!(windows, [1, 2]);
        assert_eq!(sessions(&dormant), ["b"]);
        assert_eq!(from, 500);
        // A file of nothing but open peers has nothing dormant.
        let (_, dormant, _) = sort_kept(
            KeptFile {
                boot: 1_000,
                dormant_boot: 0,
                peers: vec![kept(Some("x"), 0, 0)],
            },
            1_000,
            |_| false,
        );
        assert!(dormant.is_empty());
    }

    #[test]
    fn only_a_process_the_hook_ran_under_counts_as_its_agent() {
        // The client, then its ancestors, as the pipe found them.
        let sender = [30, 20, 10];
        assert_eq!(trusted_agent(Some(20), &sender), Some(20));
        assert_eq!(trusted_agent(Some(10), &sender), Some(10));
        // Not the hook itself, nothing it did not run under, nothing at all
        // when the pipe could not tell.
        assert_eq!(trusted_agent(Some(30), &sender), None);
        assert_eq!(trusted_agent(Some(99), &sender), None);
        assert_eq!(trusted_agent(Some(20), &[]), None);
        assert_eq!(trusted_agent(None, &sender), None);
    }

    #[test]
    fn a_peer_takes_no_session_from_a_hook_it_cannot_place() {
        let mut d = daemon();
        d.peers.push(Peer {
            window: 7,
            spawn: Spawn::default(),
        });
        d.note_peer_session(7, "planted", None);
        assert_eq!(d.peers[0].spawn.resume, None);
    }

    #[test]
    fn each_user_saves_to_files_of_their_own() {
        let dir = std::path::Path::new(r"C:\ProgramData\Daifuku");
        let a = SavedFiles::in_dir(dir, Some("S-1-5-21-1-2-3-1001"));
        let b = SavedFiles::in_dir(dir, Some("S-1-5-21-1-2-3-1002"));
        assert_eq!(a.state, dir.join("state-S-1-5-21-1-2-3-1001.json"));
        assert_eq!(a.peers, dir.join("peers-S-1-5-21-1-2-3-1001.json"));
        assert_ne!(a.state, b.state);
        assert_ne!(a.peers, b.peers);
        assert_eq!(a.legacy_state, Some(dir.join("state.json")));
        let shared = SavedFiles::in_dir(dir, None);
        assert_eq!(shared.state, dir.join("state.json"));
        assert_eq!(shared.legacy_state, None);
    }

    #[test]
    fn the_shared_state_file_is_read_only_until_the_user_has_one() {
        let dir = std::env::temp_dir().join(format!("daifuku-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let own = dir.join("state-S-1.json");
        let legacy = dir.join("state.json");
        std::fs::write(&legacy, "{}").unwrap();
        assert_eq!(state_to_read(&own, Some(&legacy)), legacy.as_path());
        std::fs::write(&own, "{}").unwrap();
        assert_eq!(state_to_read(&own, Some(&legacy)), own.as_path());
        assert_eq!(state_to_read(&own, None), own.as_path());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_replaces_the_file_whole_and_leaves_nothing_beside_it() {
        let dir = std::env::temp_dir().join(format!("daifuku-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("peers.json");
        std::fs::write(&file, "an older save that was longer than the new one").unwrap();
        replace_file(&file, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from("peers.json")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_peer_closed_while_the_daemon_runs_leaves_the_peers_file() {
        let dir = std::env::temp_dir().join(format!("daifuku-peers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("peers.json");
        let mut d = daemon();
        d.peers_file = Some(file.clone());
        // No window has this handle, so nothing about it is real but the
        // record.
        let w = 0x7fff_fff0;
        d.peers.push(Peer {
            window: w,
            spawn: Spawn {
                directory: r"C:\src".into(),
                resume: Some("abc".into()),
                ..Spawn::default()
            },
        });
        d.save_peers();
        assert_eq!(
            peer::read_kept(&std::fs::read_to_string(&file).unwrap())
                .peers
                .len(),
            1
        );
        d.window_destroyed(w);
        assert!(
            peer::read_kept(&std::fs::read_to_string(&file).unwrap())
                .peers
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_that_runs_already_is_not_restored() {
        use daifuku_core::state::HookEvent;
        let mut d = daemon();
        let spawn = |id: &str| Spawn {
            directory: r"C:\no\such\folder".into(),
            resume: Some(id.into()),
            ..Spawn::default()
        };
        d.peers.push(Peer {
            window: 9,
            spawn: spawn("in-a-peer"),
        });
        d.agents.apply(
            5,
            &HookEvent {
                session_id: "elsewhere".into(),
                hook_event_name: "Stop".into(),
                ..HookEvent::default()
            },
        );
        assert!(d.running(&spawn("in-a-peer")).is_some());
        assert!(d.running(&spawn("elsewhere")).is_some());
        assert_eq!(d.running(&spawn("free")), None);
    }

    #[test]
    fn a_dry_run_lists_the_dormant_peers_and_forget_drops_them() {
        let mut d = daemon();
        d.dormant = vec![Spawn {
            directory: r"C:\src".into(),
            resume: Some("abc".into()),
            label: Some("site".into()),
            ..Spawn::default()
        }];
        let Response::Restored { peers } = d.restore_peers(true, false) else {
            panic!("not a restore answer");
        };
        assert_eq!(peers.len(), 1);
        assert_eq!(
            (peers[0].session_id.as_str(), peers[0].window),
            ("abc", None)
        );
        assert_eq!(d.dormant.len(), 1, "a dry run keeps them");
        let Response::Restored { peers } = d.restore_peers(false, true) else {
            panic!("not a restore answer");
        };
        assert_eq!(peers.len(), 1);
        assert!(d.dormant.is_empty(), "forgotten");
    }

    #[test]
    fn a_fleet_opens_the_ones_it_names_first_and_ends_in_front() {
        let config = Config::from_json(
            r#"{"fleets":[
                {"name":"lead","count":1,"with":["crew","Notes"]},
                {"name":"crew","count":3,"with":["lead"],"hotkey":null},
                {"name":"notes","count":1,"hotkey":null}
            ]}"#,
        )
        .unwrap();
        let names = |name| {
            opening_order(&config, config.fleet(name).unwrap().clone())
                .into_iter()
                .map(|f| f.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("lead"), ["crew", "notes", "lead"]);
        // Each names the other: one level only, so neither opens twice.
        assert_eq!(names("crew"), ["lead", "crew"]);
        assert_eq!(names("notes"), ["notes"]);
    }

    #[test]
    fn the_demo_command_keeps_any_path_whole() {
        assert_eq!(
            demo_command(std::path::Path::new(
                r"C:\Program Files\Daifuku\daifuku.exe"
            )),
            r"& 'C:\Program Files\Daifuku\daifuku.exe' demo-agent {n}"
        );
        assert_eq!(
            single_quoted(r"C:\Users\O'Neill\bin"),
            r"'C:\Users\O''Neill\bin'"
        );
        assert_eq!(
            single_quoted("O\u{2019}Neill \u{2018}x\u{201A}\u{201B}"),
            "'O\u{2019}\u{2019}Neill \u{2018}\u{2018}x\u{201A}\u{201A}\u{201B}\u{201B}'"
        );
    }

    #[test]
    fn a_key_with_nothing_to_act_on_is_idle() {
        let mut d = daemon();
        assert!(d.idle(&Action::Next), "no agent waits");
        assert!(d.idle(&Action::Snap), "no fleet is open");
        assert!(d.idle(&Action::Close), "no fleet is open");
        assert!(!d.idle(&Action::Open("agents".into())));
        let mut record = OpenFleet::new("agents");
        record.slots = vec![Some(1)];
        d.fleets.push(record);
        assert!(!d.idle(&Action::Snap));
        assert!(!d.idle(&Action::Close));
    }

    #[test]
    fn the_pulse_rests_while_nobody_is_at_the_desktop() {
        let mut d = daemon();
        d.animations = true;
        d.high_contrast = false;
        d.away = true;
        d.update_pulse(true);
        assert_eq!(d.pulse_timer, None);
        d.away = false;
        d.update_pulse(true);
        assert!(d.pulse_timer.is_some(), "it runs again on return");
        d.update_pulse(false);
        assert_eq!(d.pulse_timer, None);
    }

    #[test]
    fn close_counts_only_the_terminals_that_went() {
        assert_eq!(
            closed_message(Some("e2e"), 4, 0),
            "closed 4 terminals of e2e"
        );
        assert_eq!(
            closed_message(Some("agents"), 5, 1),
            "closed 5 of 6 terminals of agents; Windows Terminal kept 1 terminal open, perhaps to ask about closing tabs"
        );
        assert_eq!(closed_message(None, 10, 0), "closed 10 terminals");
        assert_eq!(
            closed_message(None, 9, 1),
            "closed 9 of 10 terminals; Windows Terminal kept 1 terminal open, perhaps to ask about closing tabs"
        );
    }

    #[test]
    fn the_breath_never_goes_dark() {
        for i in 0..1000 {
            let k = breath(f64::from(i) * 0.013);
            assert!((0.70..=1.0).contains(&k), "{k}");
        }
        assert!(
            (breath(0.0) - 1.0).abs() < 1e-9,
            "starts at full brightness"
        );
        assert!(
            (breath(PULSE_PERIOD / 2.0) - 0.70).abs() < 1e-9,
            "bottoms out at 70 %"
        );
    }

    #[test]
    fn high_contrast_keeps_a_width_per_state() {
        let border = Border {
            state_widths: false,
            ..Border::default()
        };
        let states = [
            AgentState::Done,
            AgentState::Working,
            AgentState::Failed,
            AgentState::Waiting,
        ];
        let widths = |high_contrast| -> std::collections::BTreeSet<i32> {
            states
                .iter()
                .map(|&s| width_for(&border, s, high_contrast))
                .collect()
        };
        assert_eq!(widths(false).len(), 1, "one width with state widths off");
        assert_eq!(widths(true).len(), 4, "a width per state in high contrast");
    }

    #[test]
    fn dimming_keeps_the_hue() {
        let c = dim(daifuku_core::config::Colour::new(200, 100, 0), 0.5);
        assert_eq!((c.r, c.g, c.b), (100, 50, 0));
    }

    /// `next` from window `front`, having last brought up `last` at time `t`,
    /// while each window in the queue has shown its state since `t`.
    fn next_from(queue: &[u64], front: u64, last: Option<(u64, u32)>) -> Option<u64> {
        next_target(queue, front, last, |_| Some(1))
    }

    #[test]
    fn next_with_nobody_waiting_brings_up_nothing() {
        assert_eq!(next_from(&[], 2, Some((2, 1))), None);
    }

    #[test]
    fn next_starts_at_the_front_of_the_queue() {
        assert_eq!(next_from(&[1, 2, 3], 2, None), Some(1));
    }

    #[test]
    fn next_again_from_the_terminal_it_brought_up_moves_on() {
        assert_eq!(next_from(&[1, 2, 3], 2, Some((2, 1))), Some(3));
    }

    #[test]
    fn next_again_from_the_last_in_the_queue_wraps_to_the_first() {
        assert_eq!(next_from(&[1, 2, 3], 3, Some((3, 1))), Some(1));
    }

    #[test]
    fn next_from_another_terminal_starts_over() {
        assert_eq!(next_from(&[1, 2, 3], 2, Some((1, 1))), Some(1));
    }

    #[test]
    fn next_starts_over_once_the_agent_it_brought_up_has_moved_on() {
        let later = |w: u64| Some(if w == 2 { 2 } else { 1 });
        assert_eq!(next_target(&[1, 2, 3], 2, Some((2, 1)), later), Some(1));
    }

    #[test]
    fn next_starts_over_when_the_last_terminal_is_out_of_the_queue() {
        assert_eq!(next_from(&[1, 2, 3], 4, Some((4, 1))), Some(1));
    }

    #[test]
    fn next_with_one_waiting_stays_on_it() {
        assert_eq!(next_from(&[2], 2, Some((2, 1))), Some(2));
    }

    #[test]
    fn a_state_keeps_its_time_until_it_changes_and_gone_windows_drop_out() {
        use std::collections::{BTreeMap, HashMap};
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(1);
        let mut shown = HashMap::new();
        note_shown(
            &mut shown,
            BTreeMap::from([(1, AgentState::Waiting), (2, AgentState::Working)]),
            t0,
        );
        note_shown(
            &mut shown,
            BTreeMap::from([(1, AgentState::Waiting), (3, AgentState::Failed)]),
            t1,
        );
        assert_eq!(shown.get(&1), Some(&(AgentState::Waiting, t0)), "unchanged");
        assert_eq!(shown.get(&2), None, "gone");
        assert_eq!(shown.get(&3), Some(&(AgentState::Failed, t1)), "new");
        note_shown(&mut shown, BTreeMap::from([(1, AgentState::Done)]), t1);
        assert_eq!(shown.get(&1), Some(&(AgentState::Done, t1)), "changed");
        assert_eq!(shown.len(), 1);
    }

    #[test]
    fn a_restarted_daemon_takes_back_its_agents_and_lets_go_of_dead_windows() {
        use daifuku_core::state::HookEvent;
        let dir = std::env::temp_dir().join(format!("daifukud-saved-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("state.json");
        let waiting = |session: &str| HookEvent {
            session_id: session.into(),
            hook_event_name: "PermissionRequest".into(),
            daifuku_at: Some(clock::ticks()),
            ..HookEvent::default()
        };

        let mut before = daemon();
        before.state_file = Some(file.clone());
        before.agents.apply(7, &waiting("kept"));
        before.agents.apply(8, &waiting("closed"));
        before.note_states();
        let mut record = OpenFleet::new("agents");
        record.slots = vec![Some(7), None];
        before.fleets.push(record);
        before.save(true);
        // Nothing changed: no new write within the minute.
        std::fs::remove_file(&file).unwrap();
        before.save(false);
        let rewritten = file.exists();
        before.save(true);

        let text = std::fs::read_to_string(&file).unwrap();
        let saved: Saved = serde_json::from_str(&text).unwrap();
        let usable = saved.usable(saved::boot(wall_seconds(), clock::ticks()), clock::ticks());
        let mut after = daemon();
        after.take_back(saved.clone(), clock::ticks(), |w| w == 7);

        let mut stale = daemon();
        stale.state_file = Some(file.clone());
        let old = Saved { boot: 1, ..saved };
        std::fs::write(&file, serde_json::to_string(&old).unwrap()).unwrap();
        stale.restore();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(!rewritten, "an unchanged state was written again");
        assert!(usable, "a save just made is from this start and fresh");
        assert_eq!(after.agents.window_state(7), Some(AgentState::Waiting));
        assert_eq!(after.agents.window_state(8), None, "its window is gone");
        assert_eq!(after.agents.needs_you(), vec![7]);
        assert!(window_since(&after.agents, &after.shown_since, 7).is_some());
        assert!(
            after.fleets.is_empty(),
            "a fleet whose windows are not terminals is not open"
        );
        assert!(stale.agents.is_empty(), "a save from another start");
    }

    #[test]
    fn a_window_with_two_tabs_waits_since_the_tab_still_waiting() {
        use daifuku_core::state::HookEvent;
        use std::time::{Duration, Instant};
        let ev = |session: &str, name: &str| HookEvent {
            session_id: session.into(),
            hook_event_name: name.into(),
            ..HookEvent::default()
        };
        let note = |agents: &Agents<u64>, shown: &mut Shown, t| {
            let states = agents
                .sessions()
                .map(|(id, w, s)| ((id.to_owned(), w), s))
                .collect();
            note_shown(shown, states, t);
        };
        let t0 = Instant::now();
        let (t1, t2) = (t0 + Duration::from_secs(10), t0 + Duration::from_secs(20));
        let mut agents = Agents::new();
        let mut shown = Shown::new();
        agents.apply(1, &ev("a", "PermissionRequest"));
        note(&agents, &mut shown, t0);
        agents.apply(1, &ev("b", "PermissionRequest"));
        note(&agents, &mut shown, t1);
        assert_eq!(
            window_since(&agents, &shown, 1),
            Some(t0),
            "since the first"
        );
        // The first tab is answered; the window still waits, on the second.
        agents.apply(1, &ev("a", "PostToolUse"));
        note(&agents, &mut shown, t2);
        assert_eq!(agents.window_state(1), Some(AgentState::Waiting));
        assert_eq!(
            window_since(&agents, &shown, 1),
            Some(t1),
            "since the second"
        );
        assert_eq!(window_since(&agents, &shown, 2), None);
    }

    #[test]
    fn an_unreadable_config_is_read_once_more_per_save() {
        use std::time::{Duration, UNIX_EPOCH};
        let first = Some(UNIX_EPOCH + Duration::from_secs(1));
        let second = Some(UNIX_EPOCH + Duration::from_secs(2));
        // A sweep reads the file again when its stamp is not the one noted.
        let (mut failed, mut noted) = (None, first);
        retry_once(&mut failed, &mut noted);
        assert_eq!((failed, noted), (first, None), "read again after a failure");
        noted = first;
        retry_once(&mut failed, &mut noted);
        assert_eq!(
            (failed, noted),
            (first, first),
            "not again after failing twice on one save"
        );
        noted = second;
        retry_once(&mut failed, &mut noted);
        assert_eq!((failed, noted), (second, None), "a new save gets its retry");
    }
}
