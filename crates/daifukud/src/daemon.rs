//! The daemon's state and its message loop.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use anyhow::Context;
use daifuku_core::config::{Border, Config, Fleet, LocalTime};
use daifuku_core::peer::{self, FleetCell, Kept, KeptFile, Spawn};
use daifuku_core::protocol::{
    AgentWindow, FleetEvent, FleetQueue, FleetStatus, HandedTo, HookAnswer, HookMessage, Leftover,
    LoggedEvent, PeerStatus, QueuedTask, Request, Response, RestoredPeer, Status, StoppedTerminal,
    UsageReport,
};
use daifuku_core::saved::{self, Saved, SavedFleet, SavedShown};
use daifuku_core::state::{AgentState, Agents};
use daifuku_render::{BorderConfig, BorderManager, BorderSpec, WindowHandle};
use daifuku_win::terminal::WINDOW_CLASS;
use daifuku_win::{access, clock, dpi, paths, process, saver, window};
use windows::Win32::UI::Shell::{QUNS_NOT_PRESENT, SHQueryUserNotificationState};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, MSG, PostQuitMessage, SetTimer, TranslateMessage,
    WM_HOTKEY, WM_TIMER,
};

use crate::events::{self, Event};
use crate::fleet::{self, OpenFleet};
use crate::hotkeys::{Action, Hotkeys};
use crate::ipc::{self, Inbound, Inbox, WM_INBOX};
use crate::{queue, usage};

/// The most sessions tracked at once. A hook can report any session id it
/// likes; past this, new ones are ignored instead of growing the map forever.
const MAX_SESSIONS: usize = 512;

/// The built-in demo fleet's name.
const DEMO: &str = "demo";

/// The most events kept, the newest: as many as the terminals whose account
/// is kept, so a caller that looks every few seconds misses none.
const MAX_EVENTS: usize = 256;

/// How often dead windows are swept, in milliseconds. Destroy events do most
/// of the work; this catches the ones Windows did not deliver.
const SWEEP_MS: u32 = 2000;

/// How long `close` waits for its terminals to go. Windows Terminal closes a
/// window at once, unless it asks first whether to close all its tabs.
const CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long `stop-agents` gives what the agents started to end by itself
/// once their terminals went: Windows gives the processes of a console that
/// closes five seconds before it ends them.
const LEFTOVER_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

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
    /// Peers closed with their fleet in this start of Windows, which
    /// `daifuku restore` opens again with the dormant ones.
    closed: Vec<Spawn>,
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
    /// The last input when this daemon last opened a window, which starts
    /// the screen saver's countdown over: see [`saver`].
    saver_since: Option<u32>,
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
    /// Each account's latest limits, from its agents' status lines.
    usage: usage::Book,
    /// The agent windows whose account has reached its `warn`, as the
    /// borders show them.
    warned: Vec<u64>,
    /// The tasks handed out from the fleets' backlogs.
    queue: queue::Queue,
    /// What `daifuku events` prints, oldest first: each event with its
    /// number and when it happened, in seconds since 1970.
    events: VecDeque<(u64, u64, FleetEvent)>,
    /// The number of the latest event, 0 while there is none.
    last_event: u64,
    /// When the daemon started, in seconds since 1970.
    started_at: u64,
    /// The sessions that finished their turn in a fleet terminal holding no
    /// task, by window and session, and whether each was told idle.
    idle: Vec<((u64, String), bool)>,
    /// Something changed that may give such a session a task: a stop, a
    /// release, the config. Each is then looked at again.
    idle_stale: bool,
    /// When each backlog last changed, as last seen.
    backlog_stamps: HashMap<PathBuf, Option<std::time::SystemTime>>,
    /// Where fleets and peers open and terminals come to the front.
    desk: Box<dyn fleet::Desk>,
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

    // Only the daemon on the installed config keeps its state: one run on
    // another config, as the end-to-end tests run theirs, starts clean every
    // time and leaves the installed one's state alone. Elevated, it keeps it
    // next to the config; with the user's own rights, which cannot write
    // there, under the user's local app data.
    let keeps_state = config_override.is_none();
    let config_path = config_override
        .or_else(paths::config_file)
        .context("no ProgramData folder")?;
    let mut d = Daemon::new(config_path, elevated, events::Hooks::install());
    let own_dir = || {
        let dir = paths::state_dir(elevated)?;
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(path = %dir.display(), error = %e, "no folder for the state");
        }
        Some(dir)
    };
    if let Some(dir) = state_dir.or_else(|| keeps_state.then(own_dir).flatten()) {
        let files = SavedFiles::in_dir(&dir, daifuku_win::setup::user_sid().as_deref());
        d.state_file = Some(files.state);
        d.peers_file = Some(files.peers);
        d.legacy_state_file = files.legacy_state;
        d.usage.file = Some(files.usage);
        d.queue.file = Some(files.tasks);
    }
    d.load_config();
    d.usage.load(wall_seconds());
    d.queue
        .load(|w| window::exists(w) && window::class(w) == WINDOW_CLASS);
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
            closed: Vec::new(),
            peers_file: None,
            borders: None,
            hotkeys: Hotkeys::default(),
            elevated,
            high_contrast: access::high_contrast(),
            animations: access::animations(),
            away: away(),
            pulse_timer: None,
            saver_since: None,
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
            usage: usage::Book::default(),
            warned: Vec::new(),
            queue: queue::Queue::default(),
            events: VecDeque::new(),
            last_event: 0,
            started_at: wall_seconds(),
            idle: Vec::new(),
            idle_stale: false,
            backlog_stamps: HashMap::new(),
            desk: Box::new(fleet::Live),
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
                self.config_error = Some(format!(
                    "{}: {}",
                    self.config_path.display(),
                    daifuku_core::config::io_reason(&e)
                ));
                tracing::error!(path = %self.config_path.display(), error = %e, "config unreadable, keeping the last good one");
                retry_once(&mut self.config_failed, &mut self.config_stamp);
                return;
            }
        };
        match Config::from_json(&text) {
            Ok(c) => {
                self.config = c;
                self.config_error = None;
                self.drop_unresumed();
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
            Action::Open(name) => self.open(Some(name), true),
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
                Inbound::Hook(message, sender, asked) => {
                    // A hook that gave up waiting is told nothing, so it is
                    // handed nothing.
                    let answering = asked.as_ref().map(|(_, a)| a.as_ref()).filter(|a| a.take());
                    let task = self.hook(&message, &sender, answering);
                    if let Some((reply, _)) = asked {
                        let _ = reply.send(HookAnswer { task });
                    }
                }
                Inbound::Usage(report) => self.usage_report(&report),
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
            Request::Open { fleet } => self.open(fleet.as_deref(), false),
            Request::Snap => self.snap(),
            Request::Close { fleet } => self.close(fleet.as_deref()),
            Request::CloseAll => self.close_all(),
            Request::StopAgents { dry_run } => self.stop_agents(*dry_run),
            Request::CloseTerminal { terminal } => self.close_terminal(terminal),
            Request::Next => self.next(),
            Request::Demo => self.demo(),
            Request::Spawn(spawn) => self.spawn(spawn, false),
            Request::Label { window, label } => self.label(*window, label.as_deref()),
            Request::Restore { dry_run, forget } => self.restore_peers(*dry_run, *forget),
            Request::Status => Response::Status(self.status()),
            Request::Tasks => Response::Tasks {
                fleets: self.queues(),
            },
            Request::TaskRelease { id } => self.release_task(id),
            Request::Events { after } => self.logged_since(*after),
            Request::Reload => {
                self.idle_stale = true;
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
    ///
    /// With `answering`, the hook waits for an answer: the task the agent is
    /// handed, if any.
    fn hook(
        &mut self,
        message: &HookMessage,
        sender: &[u32],
        answering: Option<&ipc::Asking>,
    ) -> Option<String> {
        // Only a live, visible top-level window gets a frame. A pseudo console
        // window, which a hook names when its terminal was not ready yet, is
        // followed to the terminal window now; anything else is a stale handle
        // or a made-up one.
        let Some(w) = daifuku_win::console::visible_window(message.window) else {
            tracing::debug!(
                window = format!("{:#x}", message.window),
                "hook named no window to draw around"
            );
            return None;
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
            return None;
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
        self.hand_out(w, message, agent, answering?)
    }

    /// The next task of its fleet's backlog for the agent in terminal `w`,
    /// whose main thread just stopped, and what to tell it. Only to an agent
    /// the daemon itself finds in the terminal, which holds no task yet, and
    /// only while its account's `stop_at` lets it work on, and only while the
    /// hook still waits to be told.
    fn hand_out(
        &mut self,
        w: u64,
        message: &HookMessage,
        agent: Option<u32>,
        asking: &ipc::Asking,
    ) -> Option<String> {
        let event = &message.event;
        if event.hook_event_name != "Stop" || event.agent_id.is_some() {
            return None;
        }
        let fleet = self.config.fleet(&self.fleet_of(w)?)?.clone();
        let path = fleet.backlog.clone()?;
        let backlog = match queue::read(&path, self.elevated) {
            Ok(b) => b,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not read a backlog");
                return None;
            }
        };
        if self.queue.settle(&path, &backlog) {
            self.queue.save();
        }
        if self
            .queue
            .held(w, &event.session_id, |s| self.agents.knows(s))
            .is_some()
        {
            return None;
        }
        if let Some(reason) = self.stop_reason(&fleet, Some(w)) {
            tracing::info!(fleet = %fleet.name, %reason, "handed out no task");
            return None;
        }
        let task = self.queue.next(&path, &backlog)?.clone();
        // Last, as it takes a moment: the terminal's agent, not a process
        // that only names it.
        if !agent.is_some_and(|pid| in_terminal(pid, w)) {
            return None;
        }
        if !asking.answer() {
            tracing::info!(fleet = %fleet.name, "handed out no task: its hook stopped waiting");
            return None;
        }
        let claimed = self.queue.claim(queue::Claim {
            backlog: path.clone(),
            id: task.id.clone(),
            window: w,
            session: event.session_id.clone(),
            at: wall_seconds(),
        });
        if !claimed {
            tracing::warn!("handed out no task: too many are out");
            return None;
        }
        self.queue.save();
        tracing::info!(fleet = %fleet.name, task = %task.id, window = format!("{w:#x}"), "handed out a task");
        Some(hand_out_text(&path, &task))
    }

    /// Why agents of `fleet`, or the one in terminal `w`, get no task now,
    /// as their account's `stop_at` says. With rules but no numbers yet,
    /// none goes out.
    fn stop_reason(&self, fleet: &Fleet, w: Option<u64>) -> Option<String> {
        let folder = w
            .and_then(|w| self.usage.folder_of(w))
            .map(PathBuf::from)
            .or_else(|| self.config.account_of(fleet)?.claude.clone());
        let account = folder
            .as_deref()
            .and_then(|f| self.config.account_in(f))
            .map(|(_, a)| a)
            .or_else(|| self.config.account_of(fleet))?;
        if account.stop_at.is_empty() {
            return None;
        }
        let Some(limits) = folder.as_deref().and_then(|f| self.usage.limits(f)) else {
            return Some("its account has reported no usage yet".to_owned());
        };
        limits.stopped_by(&account.stop_at, local_time())
    }

    /// Every fleet's backlog, for `daifuku task`.
    fn queues(&mut self) -> Vec<FleetQueue> {
        let fleets: Vec<Fleet> = self
            .config
            .fleets
            .iter()
            .filter(|f| f.backlog.is_some())
            .cloned()
            .collect();
        let now = wall_seconds();
        let mut settled = false;
        let mut queues = Vec::new();
        for fleet in fleets {
            let Some(path) = fleet.backlog.clone() else {
                continue;
            };
            let mut q = FleetQueue {
                fleet: fleet.name.clone(),
                backlog: path.display().to_string(),
                error: None,
                tasks: Vec::new(),
                next: None,
                stopped: self.stop_reason(&fleet, None),
            };
            match queue::read(&path, self.elevated) {
                Ok(backlog) => {
                    settled |= self.queue.settle(&path, &backlog);
                    q.next = self.queue.next(&path, &backlog).map(|t| t.id.clone());
                    for task in &backlog.tasks {
                        let claim = self.queue.claims_in(&path).find(|c| c.id == task.id);
                        if claim.is_none() && !task.is_open() {
                            continue;
                        }
                        q.tasks.push(QueuedTask {
                            id: task.id.clone(),
                            title: task.title.clone(),
                            status: task.status.clone(),
                            handed_to: claim.map(|c| HandedTo {
                                window: (c.window != 0).then_some(c.window),
                                label: (c.window != 0).then(|| self.label_of(c.window)).flatten(),
                                session_id: c.session.clone(),
                                age_seconds: now.saturating_sub(c.at),
                            }),
                        });
                    }
                }
                Err(error) => q.error = Some(error),
            }
            queues.push(q);
        }
        if settled {
            self.queue.save();
        }
        queues
    }

    fn release_task(&mut self, id: &str) -> Response {
        let backlogs: Vec<PathBuf> = self
            .queue
            .backlogs_with(id)
            .into_iter()
            .map(PathBuf::from)
            .collect();
        match backlogs.as_slice() {
            [] => Response::error(format!("no task `{id}` is handed out")),
            [path] => {
                self.queue.release(path, id);
                self.queue.save();
                self.idle_stale = true;
                Response::said(format!(
                    "took {id} back; while its backlog lists it open, the next agent to stop gets it"
                ))
            }
            _ => Response::error(format!(
                "task `{id}` is handed out in {}, so none was taken back",
                self.fleets_with(&backlogs)
            )),
        }
    }

    /// The fleets whose backlog is one of `backlogs`, as "a, in b and in c",
    /// with a backlog no fleet has any more by its path.
    fn fleets_with(&self, backlogs: &[PathBuf]) -> String {
        let mut names = Vec::new();
        for path in backlogs {
            let key = daifuku_core::config::folder_key(path);
            let before = names.len();
            names.extend(
                self.config
                    .fleets
                    .iter()
                    .filter(|f| {
                        f.backlog
                            .as_ref()
                            .is_some_and(|b| daifuku_core::config::folder_key(b) == key)
                    })
                    .map(|f| f.name.clone()),
            );
            if names.len() == before {
                names.push(path.display().to_string());
            }
        }
        match names.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and in {last}", rest.join(", in ")),
            None => String::new(),
        }
    }

    /// Logs one event for `daifuku events`.
    fn log_event(&mut self, event: FleetEvent) {
        tracing::info!(event = %event.line(), "logged an event");
        self.last_event += 1;
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events
            .push_back((self.last_event, wall_seconds(), event));
    }

    /// The events logged after the one numbered `after`.
    fn logged_since(&self, after: u64) -> Response {
        let now = wall_seconds();
        Response::Events {
            events: self
                .events
                .iter()
                .filter(|(seq, ..)| *seq > after)
                .map(|(seq, at, event)| LoggedEvent {
                    seq: *seq,
                    age_seconds: now.saturating_sub(*at),
                    event: event.clone(),
                })
                .collect(),
            last: self.last_event,
            started_at: self.started_at,
        }
    }

    /// Logs what changed in the accounts' windows. A stop that went may let
    /// an agent that finished its turn take a task.
    fn watch_limits(&mut self) {
        let events = self.usage.watch(&self.config, local_time());
        for event in events {
            if !matches!(event, FleetEvent::Limit { .. }) {
                self.idle_stale = true;
            }
            self.log_event(event);
        }
    }

    /// Logs each fleet terminal's agent that finished its turn holding no
    /// task while its backlog has one it may take: as tasks go out only as
    /// agents stop, it gets that one only when it stops again. Told once per
    /// turn. A backlog is read only for a session not looked at yet, or once
    /// something changed that may give it a task.
    fn watch_idle(&mut self) {
        // By the state the terminal shows: a session done long ago in a
        // terminal that runs another is not idle.
        let done: Vec<(u64, String, String)> = self
            .agents
            .windows()
            .into_iter()
            .filter(|&(_, state)| state == AgentState::Done)
            .filter_map(|(w, state)| {
                let fleet = self.fleet_of(w)?;
                self.config.fleet(&fleet)?.backlog.as_ref()?;
                let id = shown_session(&self.agents, w, state)?;
                self.queue
                    .held(w, &id, |s| self.agents.knows(s))
                    .is_none()
                    .then_some((w, id, fleet))
            })
            .collect();
        self.idle
            .retain(|((w, id), _)| done.iter().any(|(x, y, _)| x == w && y == id));
        for path in self.config.fleets.iter().filter_map(|f| f.backlog.clone()) {
            let changed = stamp(&path);
            if self.backlog_stamps.insert(path, changed) != Some(changed) {
                self.idle_stale = true;
            }
        }
        if std::mem::take(&mut self.idle_stale) {
            self.idle.retain(|(_, told)| *told);
        }
        let mut next: HashMap<String, Option<String>> = HashMap::new();
        for (w, id, fleet) in done {
            if self.idle.iter().any(|((x, y), _)| *x == w && *y == id) {
                continue;
            }
            let Some(config) = self.config.fleet(&fleet).cloned() else {
                continue;
            };
            let task = match next.get(&fleet) {
                Some(task) => task.clone(),
                None => {
                    let task = self.next_task(&config);
                    next.insert(fleet.clone(), task.clone());
                    task
                }
            };
            let task = task.filter(|_| self.stop_reason(&config, Some(w)).is_none());
            let told = task.is_some();
            if let Some(task) = task {
                let label = self.label_of(w);
                self.log_event(FleetEvent::Idle {
                    window: w,
                    label,
                    fleet,
                    task,
                });
            }
            self.idle.push(((w, id), told));
        }
    }

    /// The id of the task `fleet`'s next agent to stop would get.
    fn next_task(&mut self, fleet: &Fleet) -> Option<String> {
        let path = fleet.backlog.as_ref()?;
        let backlog = queue::read(path, self.elevated)
            .inspect_err(|error| {
                tracing::warn!(path = %path.display(), %error, "could not read a backlog");
            })
            .ok()?;
        if self.queue.settle(path, &backlog) {
            self.queue.save();
        }
        self.queue.next(path, &backlog).map(|t| t.id.clone())
    }

    /// One status line's report of its account's limits. Kept on disk only
    /// when the numbers changed: a status line runs after every answer.
    fn usage_report(&mut self, report: &UsageReport) {
        let window = report.window.and_then(daifuku_win::console::visible_window);
        if self.usage.note(report, window, wall_seconds()) {
            self.usage.save();
            // The first numbers of an account with rules may lift its stop.
            self.idle_stale = true;
            self.watch_limits();
        }
        if self.update_warnings() {
            self.paint_borders();
        }
    }

    /// The Claude Code folder of the account terminal `w` runs on: the one
    /// its status line last reported, else its fleet's account's.
    fn account_folder(&self, w: u64) -> Option<PathBuf> {
        if let Some(folder) = self.usage.folder_of(w) {
            return Some(PathBuf::from(folder));
        }
        let record = self.fleets.iter().find(|f| f.slots.contains(&Some(w)))?;
        let fleet = self.config.fleet(&record.name)?;
        self.config.account_of(fleet)?.claude.clone()
    }

    /// Works out again which agent windows show their account's warning.
    /// True when that changed, so the borders need painting.
    fn update_warnings(&mut self) -> bool {
        let now = wall_seconds();
        let warned: Vec<u64> = self
            .agents
            .windows()
            .into_keys()
            .filter(|&w| {
                self.account_folder(w)
                    .is_some_and(|f| self.usage.warning(&self.config, &f, now))
            })
            .collect();
        if warned == self.warned {
            return false;
        }
        tracing::info!(windows = warned.len(), "account warnings changed");
        self.warned = warned;
        true
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

    /// Opens fleet `name`, or the first one, with the fleets it names. Only
    /// when its key opened it, `by_key`, does a terminal come to the front:
    /// a script that opens a fleet leaves the front as it was.
    fn open(&mut self, name: Option<&str>, by_key: bool) -> Response {
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
            match self.open_fleet(other, by_key) {
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
    /// neither the terminals that were open nor the ones just started. With
    /// `front`, its first terminal then comes to the front.
    fn open_fleet(&mut self, fleet: &daifuku_core::config::Fleet, front: bool) -> Response {
        let notes = if fleet.resume {
            self.resume_cells(fleet)
        } else {
            self.drop_unresumed();
            Vec::new()
        };
        let mut record = match self
            .fleets
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&fleet.name))
        {
            Some(i) => self.fleets.remove(i),
            None => OpenFleet::new(&fleet.name),
        };
        self.opening_windows();
        let result = self
            .desk
            .open(fleet, &self.config, &mut record, self.elevated);
        if front
            && result.is_ok()
            && let Some(first) = record.windows().next()
        {
            self.desk.focus(first);
        }
        if record.windows().next().is_some() {
            self.fleets.push(record);
        }
        self.refresh_borders();
        match result {
            Ok(message) => {
                tracing::info!(%message);
                let said: Vec<String> = std::iter::once(message).chain(notes).collect();
                Response::said(said.join("; "))
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
        self.open_fleet(&fleet, false)
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

    /// Closes the one terminal `target` names, by its window or its label,
    /// of the terminals Daifuku knows, and waits for it to go.
    fn close_terminal(&mut self, target: &str) -> Response {
        let known: Vec<(u64, Option<String>)> = self
            .known_terminals()
            .into_iter()
            .filter(|&w| window::exists(w) && window::class(w) == WINDOW_CLASS)
            .map(|w| (w, self.label_of(w)))
            .collect();
        let w = match pick_terminal(target, &known) {
            Ok(w) => w,
            Err(message) => return Response::error(message),
        };
        let name = self.label_of(w).unwrap_or_else(|| format!("{w:#010x}"));
        if !window::close(w) {
            return Response::error(format!("Windows refused to close {name}"));
        }
        let start = std::time::Instant::now();
        while window::exists(w) && start.elapsed() < CLOSE_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let closed = !window::exists(w);
        if closed && self.window_destroyed(w) {
            self.refresh_borders();
        }
        close_outcome(&name, closed)
    }

    /// Every terminal Daifuku knows: the fleets', the peers, the ones agents
    /// report from and the ones given a label.
    fn known_terminals(&self) -> std::collections::BTreeSet<u64> {
        self.fleets
            .iter()
            .flat_map(OpenFleet::windows)
            .chain(self.peers.iter().map(|p| p.window))
            .chain(self.agents.windows().into_keys())
            .chain(self.labels.keys().copied())
            .collect()
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
        self.keep_closed_peers(&asked);
        self.drop_closed_fleets();
        (closed, open)
    }

    /// Stops every agent at once: asks every fleet's terminal and every peer
    /// to close, keeps the session each was on for `daifuku restore`, and
    /// ends what their agents started that outlives the terminal. Terminals
    /// Daifuku did not open are left alone, as they may hold other tabs. A
    /// dry run only says what it would close, keep and end.
    fn stop_agents(&mut self, dry_run: bool) -> Response {
        for f in &mut self.fleets {
            f.prune();
        }
        let targets: Vec<u64> = self
            .fleets
            .iter()
            .flat_map(OpenFleet::windows)
            .chain(self.peers.iter().map(|p| p.window))
            .filter(|&w| window::exists(w) && window::class(w) == WINDOW_CLASS)
            .collect::<std::collections::BTreeSet<u64>>()
            .into_iter()
            .collect();
        let (mut terminals, keeps): (Vec<StoppedTerminal>, Vec<Option<Spawn>>) =
            targets.iter().map(|&w| self.stop_plan(w)).unzip();
        // A program with a window of its own, as a browser an agent was the
        // first to open, is for its owner to close, with all it started.
        let mut windowed = shown_window_owners();
        let found = self.started_by_agents(&targets, &windowed);
        let spared = |pid: u32, windowed: &std::collections::HashSet<u32>| {
            windowed
                .contains(&pid)
                .then(|| "it shows a window of its own".to_owned())
        };
        if dry_run {
            let leftovers = found
                .into_iter()
                .map(|(w, p, _)| Leftover {
                    reason: spared(p.pid, &windowed),
                    pid: p.pid,
                    name: p.name,
                    window: w,
                    ended: false,
                })
                .collect();
            return Response::Stopped {
                dry_run,
                terminals,
                leftovers,
            };
        }
        let asked: Vec<u64> = targets
            .iter()
            .copied()
            .filter(|&w| window::close(w))
            .collect();
        let start = std::time::Instant::now();
        while asked.iter().any(|&w| window::exists(w)) && start.elapsed() < CLOSE_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        for f in &mut self.fleets {
            f.prune();
        }
        let gone = |w: u64| asked.contains(&w) && !window::exists(w);
        let mut kept = false;
        for (t, keep) in terminals.iter_mut().zip(keeps) {
            t.closed = gone(t.window);
            t.kept &= t.closed;
            if t.closed
                && let Some(spawn) = keep
            {
                self.keep_closed(spawn);
                kept = true;
            }
        }
        if kept {
            self.save_peers();
        }
        self.keep_closed_peers(&asked);
        // Only what a terminal that went left behind: one still open, as
        // when Windows Terminal asks first about its tabs, keeps its agent.
        let found: Vec<(u64, process::Proc, u64)> =
            found.into_iter().filter(|&(w, _, _)| gone(w)).collect();
        let runs = |p: &process::Proc, at: u64| process::runs(p.pid, at);
        let start = std::time::Instant::now();
        while found.iter().any(|(_, p, at)| runs(p, *at)) && start.elapsed() < LEFTOVER_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        windowed.extend(shown_window_owners());
        let leftovers = found
            .into_iter()
            .map(|(w, p, at)| {
                let (ended, reason) = if !runs(&p, at) {
                    (false, None)
                } else if let Some(reason) = spared(p.pid, &windowed) {
                    (false, Some(reason))
                } else {
                    match process::end(p.pid, at) {
                        Ok(ended) => (ended, None),
                        Err(e) => (false, Some(daifuku_core::config::io_reason(&e))),
                    }
                };
                if ended {
                    tracing::info!(pid = p.pid, name = %p.name, "ended what an agent left running");
                }
                Leftover {
                    pid: p.pid,
                    name: p.name,
                    window: w,
                    ended,
                    reason,
                }
            })
            .collect();
        self.drop_closed_fleets();
        self.refresh_borders();
        Response::Stopped {
            dry_run,
            terminals,
            leftovers,
        }
    }

    /// What `stop-agents` says of terminal `w`, and what `daifuku restore`
    /// opens again for it once it went. A peer keeps the session it was
    /// opened on or last reported, as a fleet close keeps it. A fleet's own
    /// terminal keeps the session it shows, in that session's folder and in
    /// its cell, only when an agent reporting it runs in it and the folder
    /// is the one the fleet starts in or one inside it: restore runs in a
    /// terminal of its fleet, which may run as administrator, and any hook
    /// can name a folder.
    fn stop_plan(&self, w: u64) -> (StoppedTerminal, Option<Spawn>) {
        let cell = self.cell_of(w);
        let mut t = StoppedTerminal {
            window: w,
            label: self.label_of(w),
            fleet: cell.as_ref().map(|c| c.fleet.clone()),
            cell: cell.as_ref().map(|c| c.cell),
            session_id: None,
            directory: None,
            kept: false,
            closed: false,
        };
        if let Some(p) = self.peers.iter().find(|p| p.window == w) {
            t.session_id.clone_from(&p.spawn.resume);
            t.directory = Some(p.spawn.directory.display().to_string());
            t.kept = p.spawn.resume.as_deref().is_some_and(peer::session_id_ok);
            return (t, None);
        }
        let Some(session) = self
            .agents
            .window_state(w)
            .and_then(|state| shown_session(&self.agents, w, state))
        else {
            return (t, None);
        };
        t.directory = self
            .agents
            .session_detail(&session)
            .and_then(|d| d.cwd.clone());
        t.session_id = Some(session.clone());
        let fleet_folder = cell.as_ref().and_then(|c| {
            let f = self.config.fleet(&c.fleet)?;
            let n = usize::try_from(c.cell).ok()?;
            Some(fleet::directory(f, n, fleet::as_user(f, self.elevated)))
        });
        let keep = match (&cell, &t.directory, &fleet_folder) {
            (Some(cell), Some(directory), Some(folder))
                if peer::session_id_ok(&session)
                    && inside(folder, std::path::Path::new(directory))
                    && self
                        .agent_processes
                        .iter()
                        .filter(|(_, (_, sessions))| sessions.contains(&session))
                        .any(|(&pid, _)| in_terminal(pid, w)) =>
            {
                Some(Spawn {
                    directory: PathBuf::from(directory),
                    label: self.labels.get(&w).cloned(),
                    resume: Some(session),
                    fleet: Some(cell.fleet.clone()),
                    cell: Some(peer::Cell::At(cell.cell)),
                    ..Spawn::default()
                })
            }
            _ => None,
        };
        t.kept = keep.is_some();
        (t, keep)
    }

    /// Every process the agents in terminals `targets` started, and theirs,
    /// the agents too, each with its terminal and when it started, which
    /// tells it from a later process given the same id. Found from the
    /// agents' own processes, as their hooks named them, and only those that
    /// run in that terminal and show no window of their own, never from the
    /// terminal's: one Windows Terminal process may hold every window. What
    /// a process in `windowed` started is not followed.
    fn started_by_agents(
        &self,
        targets: &[u64],
        windowed: &std::collections::HashSet<u32>,
    ) -> Vec<(u64, process::Proc, u64)> {
        let all = process::snapshot();
        let me = process::current();
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for &w in targets {
            let sessions: std::collections::BTreeSet<&str> = self
                .agents
                .sessions()
                .filter(|&(_, window, _)| window == w)
                .map(|(id, _, _)| id)
                .collect();
            let mut roots: Vec<u32> = self
                .agent_processes
                .iter()
                .filter(|(_, (_, s))| s.iter().any(|id| sessions.contains(id.as_str())))
                .map(|(&pid, _)| pid)
                .filter(|&pid| pid != window::pid(w) && !windowed.contains(&pid))
                .filter(|&pid| in_terminal(pid, w))
                .collect();
            roots.sort_unstable();
            let spare = |pid: u32| windowed.contains(&pid);
            for (p, at) in process::descendants(&all, process::started, spare, &roots) {
                if p.pid != me && seen.insert(p.pid) {
                    out.push((w, p, at));
                }
            }
        }
        out
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
            .map(|(w, state)| {
                let session_id = shown_session(&self.agents, w, state);
                let detail = session_id
                    .as_deref()
                    .and_then(|s| self.agents.session_detail(s))
                    .cloned()
                    .unwrap_or_default();
                AgentWindow {
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
                    session_id,
                    cwd: detail.cwd,
                    last_tool: detail.last_tool,
                    last_message: detail.last_message,
                    transcript_path: detail.transcript_path,
                    account: self
                        .account_folder(w)
                        .and_then(|f| self.config.account_in(&f).map(|(name, _)| name.to_owned())),
                    warning: self.warned.contains(&w),
                }
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
            usage: self.usage.status(&self.config, wall_seconds()),
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
    /// [`MAX_SESSIONS`] processes, new ones are not either. A session is
    /// watched for the last process it reported from only: one resumed in a
    /// new process outlives the old one.
    fn watch_agent(&mut self, pid: u32, session: &str) {
        self.agent_processes.retain(|&other, (_, sessions)| {
            if other != pid {
                sessions.remove(session);
            }
            !sessions.is_empty()
        });
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
        if self.update_warnings() {
            self.paint_borders();
        }
        self.watch_limits();
        let gone = away();
        if gone != self.away {
            tracing::info!(away = gone, "the desktop was left or came back");
            self.away = gone;
            self.refresh_borders();
        }
        let dead = self.forget_dead(window::exists);
        for f in &mut self.fleets {
            f.prune();
        }
        self.drop_closed_fleets();
        if dead {
            self.refresh_borders();
        }
        self.watch_idle();
        self.watch_saver();
        self.save(false);
    }

    /// Notes the last input before a new window starts the screen saver's
    /// countdown over.
    fn opening_windows(&mut self) {
        let now = saver::Now::read();
        if now.timeout.is_some() && !now.running {
            self.saver_since = Some(now.last_input);
        }
    }

    /// Starts the screen saver a window of ours put off, once nobody has
    /// touched the machine for its timeout.
    fn watch_saver(&mut self) {
        let Some(since) = self.saver_since else {
            return;
        };
        match saver::step(since, &saver::Now::read(), self.away) {
            saver::Step::Wait => {}
            saver::Step::Forget => self.saver_since = None,
            saver::Step::Start => {
                tracing::info!("starting the screen saver a new window put off");
                saver::start();
                self.saver_since = None;
            }
        }
    }

    /// Lets go of the agents' windows and the tasks' terminals that fail
    /// `alive`, gone without the event that says so. True when an agent's
    /// window went.
    fn forget_dead(&mut self, alive: impl Fn(u64) -> bool) -> bool {
        let dead: Vec<u64> = self
            .agents
            .windows()
            .into_keys()
            .filter(|&w| !alive(w))
            .collect();
        for w in &dead {
            self.agents.forget_window(*w);
        }
        if !dead.is_empty() {
            self.note_states();
        }
        if self.queue.windows_gone(alive) {
            self.queue.save();
        }
        !dead.is_empty()
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
        if self.queue.window_closed(w) {
            self.queue.save();
        }
        self.drop_closed_fleets();
        forgot
    }

    /// What to call terminal `w`: the label `daifuku label` or `spawn` gave
    /// it, else its fleet's name and its number in the fleet, a peer in a
    /// fleet's cell included.
    fn label_of(&self, w: u64) -> Option<String> {
        if let Some(label) = self.labels.get(&w) {
            return Some(label.clone());
        }
        if let Some(label) = self
            .peers
            .iter()
            .find(|p| p.window == w)
            .and_then(|p| p.spawn.label.clone())
        {
            return Some(label);
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
        let i = self.peers.iter().position(|p| p.window == w);
        let cell = if i.is_none() {
            self.resuming_cell(w)
        } else {
            None
        };
        if i.is_none() && cell.is_none() {
            return;
        }
        if i.is_some_and(|i| self.peers[i].spawn.resume.as_deref() == Some(session)) {
            return;
        }
        if !agent.is_some_and(|a| in_terminal(a, w)) {
            // Every hook of a fleet terminal comes here until one can be
            // placed, so only a peer's is worth a warning.
            if i.is_some() {
                tracing::warn!(
                    window = format!("{w:#x}"),
                    "a hook outside the peer named its session; kept the old one"
                );
            } else {
                tracing::debug!(
                    window = format!("{w:#x}"),
                    "a hook outside the fleet terminal named its session; not kept"
                );
            }
            return;
        }
        if let Some(i) = i {
            self.peers[i].spawn.resume = Some(session.to_owned());
        } else if let Some((fleet, n, directory)) = cell {
            // A terminal of a fleet that resumes becomes a peer in its cell,
            // kept as `daifuku spawn` keeps one.
            tracing::info!(window = format!("{w:#x}"), %fleet, cell = n, "kept a fleet terminal with its session");
            self.peers.push(Peer {
                window: w,
                spawn: Spawn {
                    directory,
                    label: self.labels.remove(&w),
                    resume: Some(session.to_owned()),
                    fleet: Some(fleet),
                    cell: Some(peer::Cell::At(n)),
                    adopted: true,
                    ..Spawn::default()
                },
            });
        }
        self.save_peers();
    }

    /// The fleet window `w` is a terminal of, its cell numbered from one and
    /// the folder it started in, when that fleet resumes its sessions.
    fn resuming_cell(&self, w: u64) -> Option<(String, u32, PathBuf)> {
        self.fleets.iter().find_map(|record| {
            let slot = record.slots.iter().position(|s| *s == Some(w))?;
            let fleet = self.config.fleet(&record.name).filter(|f| f.resume)?;
            let n = slot + 1;
            // Its sessions belong to the folder it started in, which a
            // folder made or taken away since would change.
            let directory = record.folders.iter().find(|(f, _)| *f == w).map_or_else(
                || fleet::directory(fleet, n, fleet::as_user(fleet, self.elevated)),
                |(_, d)| d.clone(),
            );
            Some((fleet.name.clone(), u32::try_from(n).ok()?, directory))
        })
    }

    /// Whether fleet `name` keeps its terminals with their sessions.
    fn resumes(&self, name: Option<&str>) -> bool {
        name.and_then(|n| self.config.fleet(n))
            .is_some_and(|f| f.resume)
    }

    /// Lets go of the terminals waiting to be restored that a fleet kept
    /// for resuming, once that fleet no longer resumes or is gone.
    /// Not while the daemon runs on the built-in config, as after it started
    /// with a file it could not use: that file may resume them again.
    fn drop_unresumed(&mut self) {
        if self.on_defaults() {
            return;
        }
        let before = self.dormant.len() + self.closed.len();
        let config = &self.config;
        let keep = |s: &Spawn| {
            !s.adopted
                || s.fleet
                    .as_deref()
                    .and_then(|n| config.fleet(n))
                    .is_some_and(|f| f.resume)
        };
        self.dormant.retain(keep);
        self.closed.retain(keep);
        if self.dormant.len() + self.closed.len() != before {
            self.save_peers();
        }
    }

    /// Opens the terminals of `fleet` that wait to be restored, dormant or
    /// closed with the fleet, each in its own cell resuming its session,
    /// lowest cell first. Opening a fleet that resumes does this first, so
    /// its other cells are the only ones that start afresh. One whose cell
    /// is past the fleet's count, or holds a terminal already, stays for
    /// `daifuku restore`; one that fails to open is let go, as its cell
    /// starts afresh. Returns a line for each that did not open.
    fn resume_cells(&mut self, fleet: &daifuku_core::config::Fleet) -> Vec<String> {
        self.drop_unresumed();
        let mut waiting: Vec<(u32, Spawn)> = self
            .dormant
            .iter()
            .chain(&self.closed)
            .filter(|s| {
                s.fleet
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case(&fleet.name))
            })
            .filter_map(|s| Some((s.cell?.number()?, s.clone())))
            .collect();
        if waiting.is_empty() {
            return Vec::new();
        }
        waiting.sort_by_key(|(n, _)| *n);
        let mut notes = Vec::new();
        for (n, spawn) in waiting {
            if let Some(running) = self.running(&spawn) {
                tracing::info!(fleet = %fleet.name, %running, "not resuming a cell");
                self.forget_waiting(&spawn);
                continue;
            }
            let taken = self
                .fleets
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(&fleet.name))
                .and_then(|r| {
                    let slot = usize::try_from(n).ok()?.checked_sub(1)?;
                    r.slots.get(slot).copied().flatten()
                })
                .is_some_and(window::exists);
            if n == 0 || n > fleet.count || taken {
                notes.push(format!(
                    "cell {n} is not free for session {}, so `daifuku restore` opens it beside the fleet",
                    spawn.resume.as_deref().unwrap_or_default()
                ));
                continue;
            }
            if let Response::Error { message } = self.spawn(&spawn, true) {
                tracing::warn!(fleet = %fleet.name, cell = n, %message, "could not resume a cell");
                notes.push(format!(
                    "cell {n} did not resume and starts afresh: {message}"
                ));
            }
            self.forget_waiting(&spawn);
        }
        self.save_peers();
        notes
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
                    closed_with_fleet: false,
                    in_cell: self.cell_of(p.window),
                })
                .chain(self.dormant.iter().map(|spawn| Kept {
                    spawn: spawn.clone(),
                    window: 0,
                    boot: self.dormant_boot,
                    closed_with_fleet: false,
                    in_cell: None,
                }))
                .chain(self.closed.iter().map(|spawn| Kept {
                    spawn: spawn.clone(),
                    window: 0,
                    boot,
                    closed_with_fleet: true,
                    in_cell: None,
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

    /// The fleet and the cell terminal `w` is in, if it is in one.
    fn cell_of(&self, w: u64) -> Option<FleetCell> {
        self.fleets.iter().find_map(|f| {
            let i = f.slots.iter().position(|s| *s == Some(w))?;
            Some(FleetCell {
                fleet: f.name.clone(),
                cell: u32::try_from(i + 1).ok()?,
            })
        })
    }

    /// Reads [`Daemon::peers_file`]: a peer whose terminal is still open,
    /// as after an install, is open again, and back in the cell of its
    /// fleet it was in; one that was open when Windows last stopped is
    /// dormant, when it has a session to resume. Peers from any start of
    /// Windows before that are dropped, as is one closed in this start of
    /// Windows while no daemon ran. Written back at once, so the file says
    /// which start of Windows its dormant peers are from.
    ///
    /// A file that is there but does not read, held by another program for
    /// longer than [`read_while_held`] waits or written by a version this one
    /// cannot read, stays as it is: this run keeps its peers in memory only,
    /// as saving would lose the ones in it.
    fn load_peers(&mut self) {
        let Some(file) = self.peers_file.clone() else {
            return;
        };
        let read = match read_while_held(&file) {
            Ok(text) => peer::read_kept(&text).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => Err(e),
        };
        let kept = match read {
            Ok(kept) => kept,
            Err(e) => {
                tracing::warn!(path = %file.display(), error = %e, "the peers file does not read; it stays as it is and the peers are not saved until the daemon starts again");
                self.peers_file = None;
                return;
            }
        };
        let boot = saved::boot(wall_seconds(), clock::ticks());
        let Sorted {
            open,
            dormant,
            closed,
            from: dormant_boot,
        } = sort_kept(kept, boot, |w| {
            window::exists(w) && window::class(w) == WINDOW_CLASS
        });
        let mut seats = Vec::new();
        self.peers = open
            .into_iter()
            .map(|(window, spawn, in_cell)| {
                seats.extend(in_cell.map(|c| (window, c)));
                Peer { window, spawn }
            })
            .collect();
        self.dormant = dormant;
        self.dormant_boot = dormant_boot;
        self.closed = closed;
        self.rejoin_fleets(seats);
        self.drop_unresumed();
        self.save_peers();
        tracing::info!(
            open = self.peers.len(),
            dormant = self.dormant.len(),
            "read the peers"
        );
    }

    /// Puts each peer read back open that was in a cell of a fleet back in
    /// that cell, when the fleets taken back do not hold it already. A save
    /// of the fleets is taken back only while it is a few minutes old, the
    /// peers whenever their terminal is still open: outside its fleet, a
    /// peer would get a terminal opened over it by the fleet's key, and stay
    /// open when the fleet closes.
    fn rejoin_fleets(&mut self, seats: Vec<(u64, FleetCell)>) {
        for (w, seat) in seats {
            if self.fleets.iter().any(|f| f.slots.contains(&Some(w))) {
                continue;
            }
            let Some(definition) = self.definition(&seat.fleet) else {
                continue;
            };
            let mut record = self
                .fleets
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(&definition.name))
                .map_or_else(
                    || OpenFleet::new(&definition.name),
                    |i| self.fleets.remove(i),
                );
            let window = format!("{w:#x}");
            match fleet::rejoin(&definition, &self.config, &mut record, w, seat.cell) {
                Ok(true) => {
                    tracing::info!(%window, fleet = %definition.name, cell = seat.cell, "a peer is back in its cell");
                }
                Ok(false) => {
                    tracing::info!(%window, fleet = %definition.name, cell = seat.cell, "a peer's cell is taken, it stays beside its fleet");
                }
                Err(e) => {
                    tracing::warn!(%window, error = %e, "could not put a peer back in its fleet");
                }
            }
            self.keep_fleet(record);
        }
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
            let peers = self
                .dormant
                .iter()
                .chain(&self.closed)
                .map(|s| listed(s, None, None))
                .collect();
            if forget && !dry_run {
                self.dormant.clear();
                self.closed.clear();
                self.save_peers();
            }
            return Response::Restored { peers };
        }
        // Each stays dormant until it is open, so a daemon that stops part
        // way loses none of the rest.
        let waiting: Vec<Spawn> = self.dormant.iter().chain(&self.closed).cloned().collect();
        let mut peers: Vec<Option<RestoredPeer>> = waiting.iter().map(|_| None).collect();
        // Those that had a cell open first, so a peer without one cannot take it.
        let mut order: Vec<usize> = (0..waiting.len()).collect();
        order.sort_by_key(|&i| waiting[i].cell.is_none());
        for i in order {
            let spawn = &waiting[i];
            if let Some(running) = self.running(spawn) {
                peers[i] = Some(listed(spawn, None, Some(running)));
                self.forget_waiting(spawn);
                continue;
            }
            match self.spawn(spawn, true) {
                Response::Spawned { window, .. } => {
                    peers[i] = Some(listed(spawn, Some(window), None));
                    self.forget_waiting(spawn);
                }
                Response::Error { message } => peers[i] = Some(listed(spawn, None, Some(message))),
                _ => {}
            }
        }
        self.save_peers();
        Response::Restored {
            peers: peers.into_iter().flatten().collect(),
        }
    }

    /// Lets go of one peer waiting to be restored, dormant or closed with
    /// its fleet.
    fn forget_waiting(&mut self, spawn: &Spawn) {
        if let Some(i) = self.dormant.iter().position(|d| d == spawn) {
            self.dormant.remove(i);
        } else if let Some(i) = self.closed.iter().position(|d| d == spawn) {
            self.closed.remove(i);
        }
    }

    /// Keeps the peers among `asked`, terminals a fleet close asked to go,
    /// whose terminals went, for `daifuku restore`: closed with their fleet,
    /// not by hand. Only those with a session to resume, as a new session
    /// in their folder would not be theirs.
    fn keep_closed_peers(&mut self, asked: &[u64]) {
        let mut kept = false;
        let mut i = 0;
        while i < self.peers.len() {
            let w = self.peers[i].window;
            if asked.contains(&w) && !window::exists(w) {
                let peer = self.peers.remove(i);
                // One a fleet kept goes only while that fleet resumes.
                if peer
                    .spawn
                    .resume
                    .as_deref()
                    .is_some_and(peer::session_id_ok)
                    && (!peer.spawn.adopted || self.resumes(peer.spawn.fleet.as_deref()))
                {
                    self.keep_closed(peer.spawn);
                    kept = true;
                }
            } else {
                i += 1;
            }
        }
        if kept {
            self.save_peers();
        }
    }

    /// Keeps `spawn` for `daifuku restore`, in place of any peer kept before
    /// on its session: restore opens a session once.
    fn keep_closed(&mut self, spawn: Spawn) {
        self.dormant.retain(|d| d.resume != spawn.resume);
        self.closed.retain(|c| c.resume != spawn.resume);
        self.closed.push(spawn);
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
    ///
    /// `restoring` is set for a peer `daifuku restore` opens again. One that
    /// asks for a cell when every cell of its fleet has a terminal in it, as
    /// when the fleet was opened afresh in the meantime, then opens beside
    /// the fleet and keeps asking for its cell, rather than not at all.
    fn spawn(&mut self, spawn: &Spawn, restoring: bool) -> Response {
        if let Some(problem) = spawn.problem() {
            return Response::error(problem);
        }
        // Unnamed, the fleet of the terminal the request came from, so an
        // agent's peer signs in where the agent did.
        let named = spawn.fleet.clone().or_else(|| {
            spawn
                .from
                .and_then(|w| self.fleet_of(w))
                .filter(|name| self.config.fleet(name).is_some())
        });
        let base = match &named {
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
        // A peer in a cell joins its fleet's record, open or not yet, as does
        // one with no place of its own while the fleet is open: it takes a
        // slot of it, placed, snapped and closed with the fleet.
        let mut record = self
            .fleets
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&base.name))
            .map_or_else(|| OpenFleet::new(&base.name), |i| self.fleets.remove(i));
        let spot = match self
            .desk
            .spot(&base, &self.config, spawn, restoring, &mut record)
        {
            Ok(Some(spot)) => spot,
            Ok(None) => {
                let message = fleet::no_free_cell(&base.name, record.slots.len());
                self.keep_fleet(record);
                return Response::error(message);
            }
            Err(e) => {
                self.keep_fleet(record);
                return Response::error(format!("{e:#}"));
            }
        };
        if restoring && spawn.cell.is_some() && !matches!(spot, fleet::Spot::Cell { .. }) {
            tracing::info!(fleet = %base.name, "no free cell for a restored peer, opening it beside");
        }
        self.opening_windows();
        let opened = self
            .desk
            .spawn(&base, &self.config, spawn, self.elevated, &taken, &spot);
        // Kept with the fleet it took, so `restore` opens it in the same one.
        let mut kept = Spawn {
            fleet: named,
            from: None,
            ..spawn.clone()
        };
        let mut in_fleet = None;
        if let (Ok(window), fleet::Spot::Cell { slot, .. }) = (&opened, &spot) {
            record.slots[*slot] = Some(*window);
            let number = u32::try_from(slot + 1).unwrap_or(u32::MAX);
            kept.cell = Some(peer::Cell::At(number));
            in_fleet = Some((record.name.clone(), number));
        }
        self.keep_fleet(record);
        match opened {
            Ok(window) => {
                tracing::info!(window = format!("{window:#x}"), label = ?spawn.label, fleet = ?in_fleet, "spawned a peer");
                self.peers.push(Peer {
                    window,
                    spawn: kept,
                });
                self.save_peers();
                self.refresh_borders();
                let (fleet, cell) = in_fleet.unzip();
                let (monitor, area) = match spot {
                    fleet::Spot::Area { device, area, .. } => (Some(device), Some(area)),
                    fleet::Spot::Cell { .. } | fleet::Spot::Loose => (None, None),
                };
                Response::Spawned {
                    window,
                    label: spawn.label.clone(),
                    fleet,
                    cell,
                    monitor,
                    area,
                }
            }
            Err(e) => Response::error(format!("{e:#}")),
        }
    }

    /// The fleet window `w` is a terminal of, or the one the peer in it was
    /// opened for.
    fn fleet_of(&self, w: u64) -> Option<String> {
        self.fleets
            .iter()
            .find(|f| f.slots.contains(&Some(w)))
            .map(|f| f.name.clone())
            .or_else(|| {
                self.peers
                    .iter()
                    .find(|p| p.window == w)
                    .and_then(|p| p.spawn.fleet.clone())
            })
    }

    /// Puts a fleet's record back among the open fleets, when it has a
    /// terminal open.
    fn keep_fleet(&mut self, record: OpenFleet) {
        if record.windows().next().is_some() {
            self.fleets.push(record);
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
        self.update_warnings();
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
                // Working or done only: waiting and failed say more.
                let mut colour = match state {
                    AgentState::Working | AgentState::Done if self.warned.contains(&w) => {
                        colours.warning
                    }
                    _ => colours.of(state),
                };
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
/// first, then `fleet` itself, so with its key it ends up in front and its
/// first terminal has the focus. The others' own `with` is not followed, and a name that is
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

/// What `close --terminal` says about terminal `name`, once it `closed` or
/// the wait for it ended: a terminal still open is a failure, as nothing
/// was closed.
fn close_outcome(name: &str, closed: bool) -> Response {
    if closed {
        Response::said(format!("closed {name}"))
    } else {
        Response::error(format!(
            "asked {name} to close, and it is still open: Windows Terminal may be asking whether to close its tabs"
        ))
    }
}

/// The terminal of `known`, each with its label, that `target` names: its
/// label in any case, which must name one terminal only, else its window,
/// in decimal or as 0x and hex. A label wins, so one that looks like a
/// number, such as `2`, still names its terminal.
fn pick_terminal(target: &str, known: &[(u64, Option<String>)]) -> Result<u64, String> {
    let called: Vec<u64> = known
        .iter()
        .filter(|(_, label)| {
            label
                .as_ref()
                .is_some_and(|l| l.eq_ignore_ascii_case(target))
        })
        .map(|&(w, _)| w)
        .collect();
    match called.as_slice() {
        [w] => return Ok(*w),
        [] => {}
        many => {
            return Err(format!(
                "{} terminals are called `{target}`: name one by its window, {}",
                many.len(),
                many.iter()
                    .map(|w| format!("{w:#010x}"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ));
        }
    }
    match daifuku_core::protocol::window_number(target) {
        Some(n) if known.iter().any(|&(w, _)| w == n) => Ok(n),
        Some(n) => Err(format!("window {n:#010x} is no terminal Daifuku knows")),
        None => Err(format!("no terminal is called `{target}`")),
    }
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

/// The processes that show a top-level window.
fn shown_window_owners() -> std::collections::HashSet<u32> {
    window::top_level()
        .into_iter()
        .filter(|&w| window::is_shown(w))
        .map(window::pid)
        .collect()
}

/// Whether folder `dir` is `folder` or one inside it, as both are on disk:
/// a link inside that leads out is not.
fn inside(folder: &std::path::Path, dir: &std::path::Path) -> bool {
    match (std::fs::canonicalize(folder), std::fs::canonicalize(dir)) {
        (Ok(folder), Ok(dir)) => dir.starts_with(folder),
        _ => false,
    }
}

/// The time now, as `stop_at` rules read it.
fn local_time() -> LocalTime {
    let (weekday, minute) = clock::local_day_and_minute();
    LocalTime {
        now: wall_seconds(),
        weekday,
        minute,
    }
}

/// What an agent is told along with its task. The title is cut short, as
/// the backlog is the user's file and may hold anything.
fn hand_out_text(path: &std::path::Path, task: &daifuku_core::backlog::Task) -> String {
    format!(
        "Daifuku hands you the next task in {}: {}. {}. It is yours now: read its entry in that file and work it. It stays yours until its status there starts with done.",
        path.display(),
        task.id,
        task.title,
    )
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
/// window, the dormant ones, the ones closed with their fleet in this start
/// of Windows, and the start of Windows the dormant ones are from.
///
/// A peer is open when it was saved in this start of Windows, by the clock
/// `boot` worked out now, and its window passes `alive`. Dormant are the
/// peers of one start of Windows only: the one the file was written in when
/// that was an earlier one, as after a restart; else, as after a daemon
/// restart, the one its dormant peers were already from. A peer closed with
/// its fleet in this start of Windows stays closed with it, one from the
/// start before is dormant. Of all these, only the ones with a session to
/// resume. Everything else is dropped: older peers, and peers of this start
/// of Windows whose terminal closed otherwise.
fn sort_kept(file: KeptFile, boot: u64, alive: impl Fn(u64) -> bool) -> Sorted {
    let now = |b: u64| b.abs_diff(boot) <= 10;
    let from = if now(file.boot) {
        file.dormant_boot
    } else {
        file.boot
    };
    let mut open = Vec::new();
    let mut dormant = Vec::new();
    let mut closed = Vec::new();
    for k in file.peers {
        if k.window != 0 && k.same_boot(boot) && alive(k.window) {
            open.push((k.window, k.spawn, k.in_cell));
        } else if k.resumable() && k.closed_with_fleet && now(k.boot) {
            closed.push(k.spawn);
        } else if k.resumable() && from != 0 && !now(k.boot) && k.same_boot(from) {
            dormant.push(k.spawn);
        }
    }
    Sorted {
        open,
        dormant,
        closed,
        from,
    }
}

/// The peers of a peers file, as [`sort_kept`] sorts them.
struct Sorted {
    /// The ones still open, by their window, with the cell of a fleet each
    /// was in.
    open: Vec<(u64, Spawn, Option<FleetCell>)>,
    /// The ones open when Windows last stopped.
    dormant: Vec<Spawn>,
    /// The ones closed with their fleet in this start of Windows.
    closed: Vec<Spawn>,
    /// The start of Windows the dormant ones are from.
    from: u64,
}

/// Where one user's daemon keeps what it saves.
#[derive(Debug, PartialEq, Eq)]
struct SavedFiles {
    /// The fleets and agent states, for a restart in this start of Windows.
    state: PathBuf,
    /// The peers, for `daifuku restore`.
    peers: PathBuf,
    /// Each account's latest limits.
    usage: PathBuf,
    /// The tasks handed out from backlogs.
    tasks: PathBuf,
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
            usage: dir.join(name("usage")),
            tasks: dir.join(name("tasks")),
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
pub(crate) fn replace_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
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

/// Reads `file`, again for half a second while another program holds it, as
/// a scanner or a backup tool may for a moment while the daemon starts at
/// logon.
fn read_while_held(file: &std::path::Path) -> std::io::Result<String> {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        match std::fs::read_to_string(file) {
            // ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION.
            Err(e)
                if matches!(e.raw_os_error(), Some(32 | 33))
                    && std::time::Instant::now() < until =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            read => return read,
        }
    }
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
    fn stop_keeps_a_peer_by_its_session_and_a_fleet_terminal_only_from_its_own_agent() {
        use daifuku_core::state::HookEvent;
        let mut d = daemon();
        let mut crew = OpenFleet::new("crew");
        crew.slots = vec![Some(7)];
        d.fleets.push(crew);
        // A hook named this session for the crew's terminal, yet no agent
        // in it was seen to: restore would run whatever it names there.
        d.agents.apply(
            7,
            &HookEvent {
                session_id: "s-1".into(),
                hook_event_name: "Stop".into(),
                cwd: Some(r"C:\src\site".into()),
                ..HookEvent::default()
            },
        );
        let (t, keep) = d.stop_plan(7);
        assert_eq!(t.label.as_deref(), Some("crew 1"));
        assert_eq!((t.fleet.as_deref(), t.cell), (Some("crew"), Some(1)));
        assert_eq!(t.session_id.as_deref(), Some("s-1"));
        assert_eq!(t.directory.as_deref(), Some(r"C:\src\site"));
        assert!(!t.kept && keep.is_none());
        assert!(!t.closed);

        let peer = |window: u64, resume: Option<&str>| Peer {
            window,
            spawn: Spawn {
                directory: PathBuf::from(r"C:\src\docs"),
                label: Some("docs".into()),
                resume: resume.map(str::to_owned),
                ..Spawn::default()
            },
        };
        d.peers.push(peer(9, Some("p-1")));
        d.peers.push(peer(11, None));
        let (t, keep) = d.stop_plan(9);
        assert_eq!(t.label.as_deref(), Some("docs"));
        assert_eq!(t.session_id.as_deref(), Some("p-1"));
        assert_eq!(t.directory.as_deref(), Some(r"C:\src\docs"));
        assert!(t.kept, "a peer is kept as a fleet close keeps it");
        assert!(keep.is_none(), "the peer's own spawn is what is kept");
        let (t, _) = d.stop_plan(11);
        assert!(!t.kept, "nothing to resume");
    }

    #[test]
    fn a_session_is_kept_only_in_a_folder_inside_the_fleets_own() {
        let base = std::env::temp_dir().join(format!("daifuku-inside-{}", std::process::id()));
        let folder = base.join("site");
        let deeper = folder.join("crates");
        let other = base.join("other");
        std::fs::create_dir_all(&deeper).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        assert!(inside(&folder, &folder));
        assert!(inside(&folder, &deeper));
        assert!(inside(&folder, &deeper.join("..")));
        assert!(!inside(&folder, &other));
        assert!(!inside(&folder, &folder.join("..").join("other")));
        assert!(!inside(&folder, &base.join("site-2")), "no such folder");
        assert!(!inside(&deeper, &folder));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_kept_session_replaces_the_one_kept_before() {
        let mut d = daemon();
        let kept = |directory: &str, resume: &str| Spawn {
            directory: PathBuf::from(directory),
            resume: Some(resume.into()),
            ..Spawn::default()
        };
        d.dormant.push(kept(r"C:\old", "s-1"));
        d.closed.push(kept(r"C:\src", "s-1"));
        d.closed.push(kept(r"C:\src", "s-2"));
        d.keep_closed(kept(r"C:\src\site", "s-1"));
        assert!(d.dormant.is_empty());
        assert_eq!(
            d.closed,
            [kept(r"C:\src", "s-2"), kept(r"C:\src\site", "s-1")]
        );
    }

    #[test]
    fn stop_with_no_terminal_open_closes_and_keeps_nothing() {
        let mut d = daemon();
        let mut crew = OpenFleet::new("crew");
        crew.slots = vec![Some(7)];
        d.fleets.push(crew);
        for dry_run in [true, false] {
            let stopped = d.stop_agents(dry_run);
            assert_eq!(
                stopped,
                Response::Stopped {
                    dry_run,
                    terminals: Vec::new(),
                    leftovers: Vec::new(),
                }
            );
        }
        assert!(d.closed.is_empty());
        assert!(
            d.fleets.is_empty(),
            "a fleet with no terminal left is dropped"
        );
    }

    #[test]
    fn status_lists_what_the_shown_session_last_said() {
        use daifuku_core::state::HookEvent;
        let mut d = daemon();
        let event = |session: &str, name: &str| HookEvent {
            session_id: session.into(),
            hook_event_name: name.into(),
            ..HookEvent::default()
        };
        d.agents.apply(
            5,
            &HookEvent {
                tool_name: Some("Bash".into()),
                cwd: Some(r"C:\src\site".into()),
                transcript_path: Some(r"C:\t\asks.jsonl".into()),
                ..event("asks", "PreToolUse")
            },
        );
        d.agents.apply(
            5,
            &HookEvent {
                notification_type: Some("permission_prompt".into()),
                message: Some("may I run Bash".into()),
                ..event("asks", "Notification")
            },
        );
        // A second tab in the same window, working elsewhere.
        d.agents.apply(
            5,
            &HookEvent {
                tool_name: Some("Edit".into()),
                cwd: Some(r"C:\src\other".into()),
                ..event("works", "PreToolUse")
            },
        );
        let status = d.status();
        let a = status.agents.iter().find(|a| a.window == 5).unwrap();
        assert_eq!(a.state, AgentState::Waiting);
        assert_eq!(a.session_id.as_deref(), Some("asks"));
        assert_eq!(a.cwd.as_deref(), Some(r"C:\src\site"));
        assert_eq!(a.last_tool.as_deref(), Some("Bash"));
        assert_eq!(a.last_message.as_deref(), Some("may I run Bash"));
        assert_eq!(a.transcript_path.as_deref(), Some(r"C:\t\asks.jsonl"));
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
                let said = d.spawn(
                    &Spawn {
                        directory: directory.clone(),
                        ..Spawn::default()
                    },
                    false,
                );
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
    fn a_peer_s_folder_on_a_drive_only_the_user_has_is_found_when_it_runs_as_the_user() {
        let here = std::env::current_dir().unwrap();
        // Only where an elevated process runs beside an ordinary desktop
        // shell, as in an administrator terminal: CI's shell is elevated.
        let Some(drive) = crate::test_support::UserDrive::new(&here) else {
            return;
        };
        // A resume for a fleet with no command fails before anything starts,
        // so no terminal opens: the error says how far it got.
        for admin in [true, false] {
            let mut d = daemon();
            d.elevated = true;
            d.config.fleets[0].admin = admin;
            d.config.fleets[0].command = None;
            let said = d.spawn(
                &Spawn {
                    directory: drive.root().to_path_buf(),
                    resume: Some("abc".into()),
                    ..Spawn::default()
                },
                false,
            );
            let Response::Error { message } = said else {
                panic!("a terminal was opened: {said:?}");
            };
            if admin {
                // An administrator terminal does not have the user's drive.
                assert!(message.starts_with("there is no folder"), "{message}");
            } else {
                assert!(message.contains("opens a plain shell"), "{message}");
            }
        }
    }

    #[test]
    fn one_terminal_is_named_by_its_window_or_its_one_label() {
        let known = [
            (0x10, Some("Site QA".to_owned())),
            (0x20, Some("crew 2".to_owned())),
            (0x30, None),
            (0x40, Some("docs".to_owned())),
            (0x50, Some("Docs".to_owned())),
        ];
        assert_eq!(pick_terminal("site qa", &known), Ok(0x10));
        assert_eq!(pick_terminal("CREW 2", &known), Ok(0x20));
        assert_eq!(pick_terminal("48", &known), Ok(0x30));
        assert_eq!(pick_terminal("0x30", &known), Ok(0x30));
        assert_eq!(pick_terminal("0X10", &known), Ok(0x10));
        assert_eq!(
            pick_terminal("0x99", &known),
            Err("window 0x00000099 is no terminal Daifuku knows".into())
        );
        assert_eq!(
            pick_terminal("lead", &known),
            Err("no terminal is called `lead`".into())
        );
        // A label that reads as a number is still a label, and wins over a
        // window of that number.
        let numbered = [
            (0x2, None),
            (0x10, Some("2".to_owned())),
            (0x20, Some("0x1f".to_owned())),
        ];
        assert_eq!(pick_terminal("2", &numbered), Ok(0x10));
        assert_eq!(pick_terminal("0X1F", &numbered), Ok(0x20));
        assert_eq!(pick_terminal("0x2", &numbered), Ok(0x2));
        assert_eq!(
            pick_terminal("docs", &known),
            Err(
                "2 terminals are called `docs`: name one by its window, 0x00000040 or 0x00000050"
                    .into()
            )
        );
    }

    #[test]
    fn a_terminal_window_takes_digits_only() {
        let known = [(0x30, None), (0x10_064e, None)];
        for bad in ["+48", "0x+30", "0X+10064e", "abc"] {
            assert_eq!(
                pick_terminal(bad, &known),
                Err(format!("no terminal is called `{bad}`"))
            );
        }
    }

    #[test]
    fn a_terminal_still_open_after_the_wait_was_not_closed() {
        assert_eq!(close_outcome("site", true), Response::said("closed site"));
        assert!(matches!(
            close_outcome("site", false),
            Response::Error { message } if message.starts_with("asked site to close, and it is still open")
        ));
    }

    #[test]
    fn closing_a_terminal_daifuku_does_not_know_is_an_error_and_closes_nothing() {
        let mut d = daemon();
        assert_eq!(
            d.close_terminal("site"),
            Response::error("no terminal is called `site`")
        );
        // A real window, but none of Daifuku's: the desktop.
        // SAFETY: no arguments.
        let desktop = daifuku_win::raw(unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow()
        });
        assert!(window::exists(desktop));
        assert_eq!(
            d.close_terminal(&desktop.to_string()),
            Response::error(format!(
                "window {desktop:#010x} is no terminal Daifuku knows"
            ))
        );
        // Labelled, but not a terminal: still not closed.
        d.labels.insert(desktop, "site".into());
        assert_eq!(
            d.close_terminal("site"),
            Response::error("no terminal is called `site`")
        );
        assert!(window::exists(desktop));
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
    fn a_session_reported_again_from_a_new_process_stays_when_the_old_one_ends() {
        use daifuku_core::state::HookEvent;
        let mut d = daemon();
        let event = HookEvent {
            session_id: "again".into(),
            hook_event_name: "PermissionRequest".into(),
            ..HookEvent::default()
        };
        let mut ended = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "exit"])
            .spawn()
            .unwrap();
        let mut runs = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Killed with its console: the window goes before a sweep sees the
        // process end.
        d.agents.apply(5, &event);
        d.watch_agent(ended.id(), "again");
        ended.wait().unwrap();
        d.window_destroyed(5);
        // The same session reports from another terminal before that sweep.
        d.agents.apply(6, &event);
        d.watch_agent(runs.id(), "again");
        d.reap_agents();
        assert_eq!(
            d.agents.window_state(6),
            Some(AgentState::Waiting),
            "the process the session ran in before ended it"
        );
        let _ = runs.kill();
        let _ = runs.wait();
        d.reap_agents();
        assert_eq!(d.agents.window_state(6), None);
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
    fn a_terminal_s_fleet_is_the_one_it_is_a_cell_of_or_its_peer_was_opened_for() {
        let mut d = daemon();
        let mut record = OpenFleet::new("crew");
        record.slots = vec![Some(1), None];
        d.fleets.push(record);
        d.peers.push(Peer {
            window: 7,
            spawn: Spawn {
                fleet: Some("site".into()),
                ..Spawn::default()
            },
        });
        d.peers.push(Peer {
            window: 8,
            spawn: Spawn::default(),
        });
        assert_eq!(d.fleet_of(1).as_deref(), Some("crew"));
        assert_eq!(d.fleet_of(7).as_deref(), Some("site"));
        assert_eq!(d.fleet_of(8), None);
        assert_eq!(d.fleet_of(9), None);
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
            closed_with_fleet: false,
            in_cell: None,
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
        let Sorted {
            open,
            dormant,
            from,
            ..
        } = sort_kept(file, 1_000, |w| w == 2);
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
        let Sorted {
            open,
            dormant,
            from,
            ..
        } = sort_kept(file, 1_005, |w| w == 1 || w == 2);
        let windows: Vec<_> = open.iter().map(|(w, _, _)| *w).collect();
        assert_eq!(windows, [1, 2]);
        assert_eq!(sessions(&dormant), ["b"]);
        assert_eq!(from, 500);
        // A file of nothing but open peers has nothing dormant.
        let Sorted { dormant, .. } = sort_kept(
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
    fn only_a_fleet_that_resumes_keeps_its_terminals_with_their_sessions() {
        let mut d = daemon();
        let name = d.config.fleets[0].name.clone();
        let mut record = OpenFleet::new(&name);
        record.slots = vec![Some(5), Some(6)];
        record.folders = vec![(6, PathBuf::from(r"C:\src\b"))];
        d.fleets.push(record);
        assert!(d.resuming_cell(6).is_none());
        d.config.fleets[0].resume = true;
        // In the folder it started in, the one its sessions belong to.
        let (fleet, n, folder) = d.resuming_cell(6).unwrap();
        assert_eq!((fleet.as_str(), n), (name.as_str(), 2));
        assert_eq!(folder, PathBuf::from(r"C:\src\b"));
        assert!(d.resuming_cell(7).is_none());
        // Still only from a hook in the terminal itself.
        d.note_peer_session(6, "planted", None);
        assert!(d.peers.is_empty());
    }

    #[test]
    fn opening_a_fleet_resumes_only_its_own_cells_and_none_that_run_already() {
        let mut d = daemon();
        let kept = |fleet: &str, session: &str| Spawn {
            directory: PathBuf::from(r"C:\"),
            resume: Some(session.to_owned()),
            fleet: Some(fleet.to_owned()),
            cell: Some(peer::Cell::At(1)),
            ..Spawn::default()
        };
        d.closed = vec![kept("crew", "a"), kept("other", "b")];
        d.dormant = vec![Spawn {
            cell: None,
            ..kept("crew", "c")
        }];
        // Session `a` runs in a peer already, so nothing opens for it.
        d.peers.push(Peer {
            window: 9,
            spawn: Spawn {
                resume: Some("a".to_owned()),
                ..Spawn::default()
            },
        });
        let crew = daifuku_core::config::Fleet {
            name: "Crew".to_owned(),
            count: 2,
            resume: true,
            ..daifuku_core::config::Fleet::default()
        };
        assert!(d.resume_cells(&crew).is_empty());
        let waiting: Vec<_> = d
            .closed
            .iter()
            .chain(&d.dormant)
            .filter_map(|s| s.resume.as_deref())
            .collect();
        assert_eq!(waiting, ["b", "c"]);
    }

    #[test]
    fn a_kept_cell_past_the_fleet_s_count_stays_for_restore() {
        let mut d = daemon();
        d.closed = vec![Spawn {
            directory: PathBuf::from(r"C:\"),
            resume: Some("far".to_owned()),
            fleet: Some("crew".to_owned()),
            cell: Some(peer::Cell::At(3)),
            adopted: true,
            ..Spawn::default()
        }];
        let crew = daifuku_core::config::Fleet {
            name: "crew".to_owned(),
            count: 2,
            resume: true,
            ..daifuku_core::config::Fleet::default()
        };
        let notes = d.resume_cells(&crew);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("cell 3"), "{notes:?}");
        assert_eq!(d.closed.len(), 1);
    }

    #[test]
    fn what_a_fleet_kept_goes_once_it_no_longer_resumes() {
        let mut d = daemon();
        d.config.fleets[0].name = "crew".to_owned();
        let kept = |session: &str, adopted: bool| Spawn {
            directory: PathBuf::from(r"C:\"),
            resume: Some(session.to_owned()),
            fleet: Some("crew".to_owned()),
            cell: Some(peer::Cell::At(1)),
            adopted,
            ..Spawn::default()
        };
        d.dormant = vec![kept("a", true), kept("b", false)];
        d.closed = vec![kept("c", true)];
        d.config.fleets[0].resume = true;
        d.drop_unresumed();
        assert_eq!((d.dormant.len(), d.closed.len()), (2, 1));
        // Without `resume`, only what the fleet kept itself goes: a peer
        // `daifuku spawn` put in a cell stays for `daifuku restore`.
        d.config.fleets[0].resume = false;
        d.drop_unresumed();
        assert_eq!(d.dormant, [kept("b", false)]);
        assert!(d.closed.is_empty());
    }

    #[test]
    fn each_user_saves_to_files_of_their_own() {
        let dir = std::path::Path::new(r"C:\ProgramData\Daifuku");
        let a = SavedFiles::in_dir(dir, Some("S-1-5-21-1-2-3-1001"));
        let b = SavedFiles::in_dir(dir, Some("S-1-5-21-1-2-3-1002"));
        assert_eq!(a.state, dir.join("state-S-1-5-21-1-2-3-1001.json"));
        assert_eq!(a.peers, dir.join("peers-S-1-5-21-1-2-3-1001.json"));
        assert_eq!(a.tasks, dir.join("tasks-S-1-5-21-1-2-3-1001.json"));
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
    fn a_peer_in_a_cell_is_kept_with_its_fleet_and_cell_and_read_back_so() {
        let dir = std::env::temp_dir().join(format!("daifuku-cells-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("peers.json");
        let mut d = daemon();
        d.peers_file = Some(file.clone());
        let (inside, beside) = (0x7fff_ffe0, 0x7fff_ffe4);
        let mut crew = OpenFleet::new("crew");
        crew.slots = vec![None, Some(inside)];
        d.fleets.push(crew);
        for window in [inside, beside] {
            d.peers.push(Peer {
                window,
                spawn: Spawn {
                    directory: r"C:\src".into(),
                    resume: Some(format!("s{window:x}")),
                    cell: Some(peer::Cell::At(2)),
                    ..Spawn::default()
                },
            });
        }
        d.save_peers();
        let read = peer::read_kept(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let cells: Vec<_> = read.peers.iter().map(|k| k.in_cell.clone()).collect();
        let seat = FleetCell {
            fleet: "crew".into(),
            cell: 2,
        };
        // Only the one the fleet holds; the other asked for the cell too,
        // but opened beside the fleet.
        assert_eq!(cells, [Some(seat.clone()), None]);
        let Sorted { open, .. } = sort_kept(read.clone(), read.boot, |_| true);
        let seats: Vec<_> = open.into_iter().map(|(w, _, c)| (w, c)).collect();
        assert_eq!(seats, [(inside, Some(seat)), (beside, None)]);
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
                .unwrap()
                .peers
                .len(),
            1
        );
        d.window_destroyed(w);
        assert!(
            peer::read_kept(&std::fs::read_to_string(&file).unwrap())
                .unwrap()
                .peers
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_peers_file_that_does_not_read_is_never_saved_over() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("daifuku-unread-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("peers.json");
        let spawn_one = |d: &mut Daemon| {
            d.peers.push(Peer {
                window: 0x7fff_ffe8,
                spawn: Spawn {
                    directory: r"C:\src".into(),
                    resume: Some("abc".into()),
                    ..Spawn::default()
                },
            });
            d.save_peers();
        };
        // Written by a later version, with a cell this one does not know.
        let foreign = r#"{"boot": 1, "dormant_boot": 1, "peers": [{"spawn": {"directory": "C:\\old", "resume": "def", "cell": "last"}, "window": 0, "boot": 1}]}"#;
        // Whole, but held by another program while the daemon starts.
        let dormant = KeptFile {
            boot: 1,
            dormant_boot: 1,
            peers: vec![Kept {
                spawn: Spawn {
                    directory: r"C:\old".into(),
                    resume: Some("def".into()),
                    ..Spawn::default()
                },
                window: 0,
                boot: 1,
                closed_with_fleet: false,
                in_cell: None,
            }],
        };
        let held = serde_json::to_string(&dormant).unwrap();
        for (text, lock) in [(foreign, false), (held.as_str(), true)] {
            std::fs::write(&file, text).unwrap();
            let mut d = daemon();
            d.peers_file = Some(file.clone());
            let holder = lock.then(|| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(&file)
                    .unwrap()
            });
            d.load_peers();
            drop(holder);
            spawn_one(&mut d);
            assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_peers_file_held_for_a_moment_at_start_is_read() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("daifuku-held-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("peers.json");
        let kept = KeptFile {
            boot: 1,
            dormant_boot: 1,
            peers: vec![Kept {
                spawn: Spawn {
                    directory: r"C:\old".into(),
                    resume: Some("def".into()),
                    ..Spawn::default()
                },
                window: 0,
                boot: 1,
                closed_with_fleet: false,
                in_cell: None,
            }],
        };
        std::fs::write(&file, serde_json::to_string(&kept).unwrap()).unwrap();
        // A scanner or a backup tool lets go of it soon after the daemon
        // starts at logon.
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&file)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(holder);
        });
        let mut d = daemon();
        d.peers_file = Some(file.clone());
        d.load_peers();
        release.join().unwrap();
        assert_eq!(d.dormant.len(), 1);
        assert!(d.peers_file.is_some());
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
    fn peers_closed_with_their_fleet_stay_restorable_until_the_next_restart() {
        let closed = |resume, boot| Kept {
            closed_with_fleet: true,
            ..kept(resume, 0, boot)
        };
        let file = KeptFile {
            boot: 1_000,
            dormant_boot: 500,
            peers: vec![
                closed(Some("crew"), 1_000),
                closed(None, 1_000),
                kept(Some("b"), 0, 500),
            ],
        };
        // A daemon restart in the same start of Windows keeps them closed
        // with their fleet, beside the dormant ones.
        let Sorted {
            dormant,
            closed: kept_closed,
            ..
        } = sort_kept(file.clone(), 1_005, |_| false);
        assert_eq!(sessions(&dormant), ["b"]);
        assert_eq!(sessions(&kept_closed), ["crew"]);
        // After a restart they are dormant, the older ones gone.
        let Sorted {
            dormant,
            closed: kept_closed,
            from,
            ..
        } = sort_kept(file, 2_000, |_| false);
        assert_eq!(sessions(&dormant), ["crew"]);
        assert!(kept_closed.is_empty());
        assert_eq!(from, 1_000);
    }

    #[test]
    fn a_fleet_close_keeps_its_peers_with_a_session_for_restore() {
        let mut d = daemon();
        // Handles no window has: as terminals that went.
        let (gone, no_session, other) = (0x7fff_fff1, 0x7fff_fff2, 0x7fff_fff3);
        for (window, resume) in [(gone, Some("s1")), (no_session, None), (other, Some("s3"))] {
            d.peers.push(Peer {
                window,
                spawn: kept(resume, window, 0).spawn,
            });
        }
        d.keep_closed_peers(&[gone, no_session]);
        assert_eq!(sessions(&d.closed), ["s1"]);
        let left: Vec<u64> = d.peers.iter().map(|p| p.window).collect();
        assert_eq!(left, [other], "only the fleet's peers leave");
        // A dry run lists them with the dormant ones, and forget drops both.
        let Response::Restored { peers } = d.restore_peers(true, false) else {
            panic!("not a restore answer");
        };
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].session_id, "s1");
        d.restore_peers(false, true);
        assert!(d.closed.is_empty() && d.dormant.is_empty());
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
    fn the_sweep_takes_a_task_off_a_terminal_that_is_gone() {
        let mut d = daemon();
        let claim = |id: &str, window, session: &str| queue::Claim {
            backlog: PathBuf::from(r"C:\work\BACKLOG.md"),
            id: id.to_owned(),
            window,
            session: session.to_owned(),
            at: 1,
        };
        d.queue.claim(claim("A1", 7, "s1"));
        d.queue.claim(claim("A2", 8, "s2"));
        assert!(!d.forget_dead(|w| w == 7), "no agent's window went");
        assert!(d.queue.held(7, "other", |_| false).is_some());
        assert!(d.queue.held(8, "other", |_| false).is_none());
        assert!(d.queue.held(9, "s2", |_| false).is_some());
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

    #[test]
    fn an_agent_done_with_no_task_while_its_backlog_has_one_is_told_idle_once_a_turn() {
        use daifuku_core::state::HookEvent;
        let dir = std::env::temp_dir().join(format!("daifuku-idle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let backlog = dir.join("BACKLOG.md");
        std::fs::write(
            &backlog,
            "### A1. Write the guide | owner: | status: open
",
        )
        .unwrap();
        let mut d = daemon();
        d.config.fleets[0].backlog = Some(backlog);
        let fleet = d.config.fleets[0].name.clone();
        let mut record = OpenFleet::new(&fleet);
        record.slots = vec![Some(1), Some(2)];
        d.fleets.push(record);
        let event = |session: &str, name: &str| HookEvent {
            session_id: session.into(),
            hook_event_name: name.into(),
            ..HookEvent::default()
        };
        d.agents.apply(1, &event("a", "Stop"));
        d.agents.apply(2, &event("b", "UserPromptSubmit"));
        d.watch_idle();
        d.watch_idle();
        let idle = FleetEvent::Idle {
            window: 1,
            label: Some(format!("{fleet} 1")),
            fleet: fleet.clone(),
            task: "A1".into(),
        };
        let Response::Events { events, last, .. } = d.logged_since(0) else {
            panic!("not events");
        };
        assert_eq!(last, 1, "told once, and the working agent not at all");
        assert_eq!(events[0].event, idle);
        // Its next turn ends idle again.
        d.agents.apply(1, &event("a", "UserPromptSubmit"));
        d.watch_idle();
        d.agents.apply(1, &event("a", "Stop"));
        d.watch_idle();
        let Response::Events { events, last, .. } = d.logged_since(1) else {
            panic!("not events");
        };
        assert_eq!((last, events.len()), (2, 1));
        assert_eq!(events[0].seq, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_out_in_two_fleets_is_not_released_by_its_id_alone() {
        let mut d = daemon();
        let mut docs = d.config.fleets[0].clone();
        docs.name = "docs".into();
        docs.backlog = Some(PathBuf::from(r"C:\docs\BACKLOG.md"));
        d.config.fleets[0].backlog = Some(PathBuf::from(r"C:\work\BACKLOG.md"));
        let crew = d.config.fleets[0].name.clone();
        d.config.fleets.push(docs);
        let claim = |backlog: &str, id: &str, window| queue::Claim {
            backlog: PathBuf::from(backlog),
            id: id.into(),
            window,
            session: format!("s{window}"),
            at: 1,
        };
        d.queue.claim(claim(r"C:\work\BACKLOG.md", "A1", 1));
        d.queue.claim(claim(r"C:\docs\BACKLOG.md", "A1", 2));
        d.queue.claim(claim(r"C:\docs\BACKLOG.md", "A2", 3));
        let Response::Error { message } = d.release_task("A1") else {
            panic!("released an id out in two fleets");
        };
        assert_eq!(
            message,
            format!("task `A1` is handed out in {crew} and in docs, so none was taken back")
        );
        assert!(
            d.queue.held(1, "-", |_| false).is_some() && d.queue.held(2, "-", |_| false).is_some()
        );
        assert!(matches!(d.release_task("A2"), Response::Ok { .. }));
        assert!(d.queue.held(3, "-", |_| false).is_none());
        assert!(
            d.queue.held(2, "-", |_| false).is_some(),
            "the other A1 stays out"
        );
        assert!(matches!(d.release_task("A9"), Response::Error { .. }));
        // A backlog no fleet names any more goes by its path.
        d.queue.claim(claim(r"C:\gone\BACKLOG.md", "A1", 4));
        let Response::Error { message } = d.release_task("A1") else {
            panic!("released an id out in three backlogs");
        };
        assert_eq!(
            message,
            format!(
                r"task `A1` is handed out in {crew}, in docs and in C:\gone\BACKLOG.md, so none was taken back"
            )
        );
    }

    #[test]
    fn the_event_log_keeps_the_newest() {
        let mut d = daemon();
        for window in 0..u64::try_from(MAX_EVENTS).unwrap() + 3 {
            d.log_event(FleetEvent::Idle {
                window,
                label: None,
                fleet: "crew".into(),
                task: "A1".into(),
            });
        }
        let Response::Events { events, last, .. } = d.logged_since(0) else {
            panic!("not events");
        };
        assert_eq!(events.len(), MAX_EVENTS);
        assert_eq!(events[0].seq, 4);
        assert_eq!(last, events[MAX_EVENTS - 1].seq);
        assert_eq!(events[MAX_EVENTS - 1].seq, events[0].seq + 255);
    }

    /// A desktop of one monitor where every empty cell and every peer gets a
    /// made-up window, and the windows brought to the front and the places
    /// peers open in are noted.
    struct StandIn {
        next: u64,
        focused: std::rc::Rc<std::cell::RefCell<Vec<u64>>>,
        spots: Noted<fleet::Spot>,
    }

    /// What a stand-in notes, shared with the test.
    type Noted<T> = std::rc::Rc<std::cell::RefCell<Vec<T>>>;

    /// The stand-in desktop's monitor.
    fn stand_in_monitor() -> daifuku_core::monitor::MonitorInfo {
        daifuku_core::monitor::MonitorInfo {
            device: "DISPLAY1".into(),
            bounds: daifuku_core::Rect::new(0, 0, 3840, 2160),
            work: daifuku_core::Rect::new(0, 0, 3840, 2112),
            dpi: 144,
            primary: true,
        }
    }

    impl StandIn {
        fn window(&mut self) -> u64 {
            self.next += 1;
            self.next
        }
    }

    impl fleet::Desk for StandIn {
        fn open(
            &mut self,
            fleet: &daifuku_core::config::Fleet,
            _: &Config,
            record: &mut OpenFleet,
            _: bool,
        ) -> anyhow::Result<String> {
            record
                .slots
                .resize(usize::try_from(fleet.count).unwrap(), None);
            for slot in &mut record.slots {
                if slot.is_none() {
                    *slot = Some(self.window());
                }
            }
            Ok(format!("opened {}", fleet.name))
        }

        fn spot(
            &mut self,
            base: &daifuku_core::config::Fleet,
            config: &Config,
            spawn: &Spawn,
            beside: bool,
            record: &mut OpenFleet,
        ) -> anyhow::Result<Option<fleet::Spot>> {
            fleet::spot_on(
                base,
                config,
                spawn,
                beside,
                record,
                &[stand_in_monitor()],
                (0, 0),
            )
        }

        fn spawn(
            &mut self,
            _: &daifuku_core::config::Fleet,
            _: &daifuku_core::config::Config,
            _: &Spawn,
            _: bool,
            _: &[u64],
            spot: &fleet::Spot,
        ) -> anyhow::Result<u64> {
            self.spots.borrow_mut().push(spot.clone());
            Ok(self.window())
        }

        fn focus(&mut self, w: u64) -> bool {
            self.focused.borrow_mut().push(w);
            true
        }
    }

    /// A daemon on `config` whose desktop is a stand-in, and the windows it
    /// brings to the front.
    fn on_stand_in(config: &str) -> (Daemon, std::rc::Rc<std::cell::RefCell<Vec<u64>>>) {
        let (d, focused, _) = with_spots(config);
        (d, focused)
    }

    /// [`on_stand_in`], with the places peers open in.
    fn with_spots(config: &str) -> (Daemon, Noted<u64>, Noted<fleet::Spot>) {
        let mut d = daemon();
        d.config = Config::from_json(config).unwrap();
        let focused = std::rc::Rc::default();
        let spots = std::rc::Rc::default();
        d.desk = Box::new(StandIn {
            next: 0x100,
            focused: std::rc::Rc::clone(&focused),
            spots: std::rc::Rc::clone(&spots),
        });
        (d, focused, spots)
    }

    const LEAD_AND_CREW: &str = r#"{"fleets":[
        {"name":"lead","count":1,"with":["crew"],"hotkey":"ctrl + alt + return"},
        {"name":"crew","count":3,"hotkey":"ctrl + alt + w"}
    ]}"#;

    #[test]
    fn open_from_a_command_leaves_the_front_alone() {
        let (mut d, focused) = on_stand_in(LEAD_AND_CREW);
        let said = d.control(&Request::Open {
            fleet: Some("lead".into()),
        });
        assert!(matches!(said, Response::Ok { .. }), "{said:?}");
        assert_eq!(d.fleets.len(), 2);
        // Open again, as a script does to bring a fleet back.
        let said = d.control(&Request::Open { fleet: None });
        assert!(matches!(said, Response::Ok { .. }), "{said:?}");
        assert_eq!(*focused.borrow(), Vec::<u64>::new());
    }

    #[test]
    fn spawn_leaves_the_front_alone() {
        let (mut d, focused) = on_stand_in(LEAD_AND_CREW);
        let said = d.control(&Request::Spawn(Spawn {
            directory: std::env::current_dir().unwrap(),
            label: Some("peer".into()),
            ..Spawn::default()
        }));
        assert!(matches!(said, Response::Spawned { .. }), "{said:?}");
        assert_eq!(*focused.borrow(), Vec::<u64>::new());
    }

    #[test]
    fn restore_leaves_the_front_alone() {
        let (mut d, focused) = on_stand_in(LEAD_AND_CREW);
        d.dormant = vec![Spawn {
            directory: std::env::current_dir().unwrap(),
            resume: Some("abc".into()),
            ..Spawn::default()
        }];
        let said = d.control(&Request::Restore {
            dry_run: false,
            forget: false,
        });
        let Response::Restored { peers } = said else {
            panic!("{said:?}");
        };
        assert!(peers[0].window.is_some(), "{peers:?}");
        assert_eq!(*focused.borrow(), Vec::<u64>::new());
    }

    #[test]
    fn a_fleet_s_key_brings_its_first_terminal_to_the_front() {
        let (mut d, focused) = on_stand_in(LEAD_AND_CREW);
        d.hotkey(&Action::Open("lead".into()));
        let lead = d.fleets.iter().find(|f| f.name == "lead").unwrap();
        assert_eq!(focused.borrow().last(), lead.windows().next().as_ref());
    }

    const CREW: &str = r#"{"fleets":[
        {"name":"crew","count":3,"area":"right","shape":{"columns":1,"rows":3}}
    ]}"#;

    /// A spawn into fleet `crew` with nothing else asked for but `area`.
    fn crew_peer(area: Option<daifuku_core::config::Area>) -> Request {
        Request::Spawn(Spawn {
            directory: std::env::current_dir().unwrap(),
            fleet: Some("crew".into()),
            area,
            ..Spawn::default()
        })
    }

    /// Where the crew's cells are on the stand-in monitor, and its half.
    fn crew_places(d: &Daemon) -> (Vec<daifuku_core::Rect>, fleet::Spot) {
        use daifuku_core::config::Area;
        use daifuku_core::grid::Grid;
        let crew = d.config.fleet("crew").unwrap();
        let work = stand_in_monitor().work;
        let cells = Grid::plan(3, Area::Right.within(work), d.config.gaps, crew.shape);
        let half = Grid::plan(1, Area::Right.within(work), d.config.gaps, None)[0];
        let half = fleet::Spot::Area {
            device: stand_in_monitor().device,
            area: Area::Right,
            rect: half,
        };
        (cells, half)
    }

    #[test]
    fn a_spawn_without_a_cell_opens_in_a_free_cell_of_its_open_fleet() {
        let (mut d, _, spots) = with_spots(CREW);
        d.control(&Request::Open {
            fleet: Some("crew".into()),
        });
        d.fleets[0].slots[1] = None;
        let said = d.control(&crew_peer(None));
        let Response::Spawned {
            window,
            fleet,
            cell,
            monitor,
            area,
            ..
        } = said
        else {
            panic!("{said:?}");
        };
        assert_eq!(
            (fleet.as_deref(), cell, monitor, area),
            (Some("crew"), Some(2), None, None)
        );
        assert_eq!(d.fleets[0].slots[1], Some(window), "it joined the crew");
        let (cells, _) = crew_places(&d);
        assert_eq!(
            *spots.borrow(),
            vec![fleet::Spot::Cell {
                slot: 1,
                rect: cells[1]
            }]
        );
    }

    #[test]
    fn a_spawn_without_a_cell_opens_on_its_fleet_s_half_when_the_fleet_is_closed_or_full() {
        let (mut d, _, spots) = with_spots(CREW);
        let (_, half) = crew_places(&d);
        let said = d.control(&crew_peer(None));
        let Response::Spawned {
            cell,
            monitor,
            area,
            ..
        } = said
        else {
            panic!("{said:?}");
        };
        assert_eq!(
            (cell, monitor.as_deref(), area),
            (
                None,
                Some("DISPLAY1"),
                Some(daifuku_core::config::Area::Right)
            )
        );
        assert!(d.fleets.is_empty(), "the closed crew stays closed");
        d.control(&Request::Open {
            fleet: Some("crew".into()),
        });
        let full = d.fleets[0].slots.clone();
        let said = d.control(&crew_peer(None));
        assert!(
            matches!(said, Response::Spawned { cell: None, .. }),
            "{said:?}"
        );
        assert_eq!(d.fleets[0].slots, full, "the full crew stays as it was");
        assert_eq!(*spots.borrow(), vec![half.clone(), half]);
    }

    #[test]
    fn a_restored_peer_without_a_cell_leaves_the_cell_another_one_had() {
        let (mut d, _, _) = with_spots(CREW);
        d.control(&Request::Open {
            fleet: Some("crew".into()),
        });
        d.fleets[0].slots[0] = None;
        let peer = |id: &str, cell| Spawn {
            directory: std::env::current_dir().unwrap(),
            fleet: Some("crew".into()),
            resume: Some(id.into()),
            cell,
            ..Spawn::default()
        };
        d.dormant = vec![
            peer("aaa", None),
            peer("bbb", Some(daifuku_core::peer::Cell::At(1))),
        ];
        let said = d.control(&Request::Restore {
            dry_run: false,
            forget: false,
        });
        let Response::Restored { peers } = said else {
            panic!("{said:?}");
        };
        let ids: Vec<&str> = peers.iter().map(|p| p.session_id.as_str()).collect();
        assert_eq!(ids, ["aaa", "bbb"], "listed in the order they were saved");
        assert_eq!(d.fleets[0].slots[0], peers[1].window, "{peers:?}");
    }

    #[test]
    fn an_area_a_spawn_asks_for_wins_over_its_open_fleet() {
        use daifuku_core::config::Area;
        let (mut d, _, spots) = with_spots(CREW);
        d.control(&Request::Open {
            fleet: Some("crew".into()),
        });
        d.fleets[0].slots[1] = None;
        let kept = d.fleets[0].slots.clone();
        let said = d.control(&crew_peer(Some(Area::Left)));
        assert!(
            matches!(said, Response::Spawned { cell: None, .. }),
            "{said:?}"
        );
        assert_eq!(d.fleets[0].slots, kept, "it stays out of the crew");
        let left = daifuku_core::grid::Grid::plan(
            1,
            Area::Left.within(stand_in_monitor().work),
            d.config.gaps,
            None,
        )[0];
        assert_eq!(
            *spots.borrow(),
            vec![fleet::Spot::Area {
                device: "DISPLAY1".into(),
                area: Area::Left,
                rect: left,
            }]
        );
    }
}
