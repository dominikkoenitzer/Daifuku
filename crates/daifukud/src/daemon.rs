//! The daemon's state and its message loop.

use std::path::PathBuf;

use anyhow::Context;
use daifuku_core::config::{Border, Config};
use daifuku_core::protocol::{AgentWindow, FleetStatus, HookMessage, Request, Response, Status};
use daifuku_core::state::{AgentState, Agents};
use daifuku_render::{BorderConfig, BorderManager, BorderSpec, WindowHandle};
use daifuku_win::{access, dpi, paths, process, window};
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

/// How often a waiting border's pulse repaints: twenty times a second, which
/// is smooth for a slow breath and nothing for the compositor.
const PULSE_MS: u32 = 50;

/// One breath of the pulse, in seconds.
const PULSE_PERIOD: f64 = 1.6;

struct Daemon {
    config: Config,
    config_path: PathBuf,
    config_error: Option<String>,
    agents: Agents<u64>,
    fleets: Vec<OpenFleet>,
    borders: Option<BorderManager>,
    hotkeys: Hotkeys,
    elevated: bool,
    /// Windows' own settings, read at start and on every sweep.
    high_contrast: bool,
    animations: bool,
    /// The pulse timer, running only while an agent waits.
    pulse_timer: Option<usize>,
    /// The window event hooks. The ones borders follow their windows by are
    /// in only while an agent has a window and borders are drawn.
    hooks: events::Hooks,
    started: std::time::Instant,
    /// When the config file last changed, to reload it without being asked.
    config_stamp: Option<std::time::SystemTime>,
    /// The stamp of the config file when reading it last failed, so each
    /// version of the file is read at most twice.
    config_failed: Option<std::time::SystemTime>,
    /// The monitors as last seen, to put fleets back when they change.
    monitors: Vec<daifuku_core::monitor::MonitorInfo>,
    /// Since when each window has shown its state, for `daifuku status`.
    shown_since: std::collections::HashMap<u64, (AgentState, std::time::Instant)>,
    /// The terminal `next` last brought to the front, and since when its
    /// agent had shown the state it was brought up for, so that pressing it
    /// again from there moves on instead of starting over.
    last_next: Option<(u64, std::time::Instant)>,
}

/// Runs the daemon until it is told to stop.
///
/// Logging is set up by the caller, so an error this returns still reaches
/// the log before the writer stops.
pub fn run(config_override: Option<PathBuf>) -> anyhow::Result<()> {
    dpi::per_monitor_v2();
    let elevated = process::current_is_elevated();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        elevated,
        "daifukud starting"
    );

    // SAFETY: no arguments.
    let main_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    let inbox = ipc::start(main_thread).context(
        "could not open Daifuku's pipes; another daifukud may already run in this session",
    )?;

    let config_path = config_override
        .or_else(paths::config_file)
        .context("no ProgramData folder")?;
    let mut d = Daemon {
        config: Config::default(),
        config_path,
        config_error: None,
        agents: Agents::new(),
        fleets: Vec::new(),
        borders: None,
        hotkeys: Hotkeys::default(),
        elevated,
        high_contrast: access::high_contrast(),
        animations: access::animations(),
        pulse_timer: None,
        hooks: events::Hooks::install(),
        started: std::time::Instant::now(),
        config_stamp: None,
        config_failed: None,
        monitors: daifuku_win::monitor::monitors(),
        shown_since: std::collections::HashMap::new(),
        last_next: None,
    };
    d.load_config();
    d.borders = BorderManager::new(BorderConfig::from(&d.config.border))
        .inspect_err(|e| tracing::error!(error = %e, "no borders"))
        .ok();
    d.hotkeys.register(&d.config);
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
                    d.refresh_borders();
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
    if let Some(borders) = d.borders.take() {
        borders.stop();
    }
    // `daifuku stop` waits for its "stopping"; exiting first would cut it off.
    inbox.wait_for_stop_reply(std::time::Duration::from_secs(1));
    tracing::info!("daifukud stopped");
    Ok(())
}

impl Daemon {
    fn load_config(&mut self) {
        self.config_stamp = stamp(&self.config_path);
        let text = match std::fs::read_to_string(&self.config_path) {
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

    fn hotkey(&mut self, action: &Action) {
        tracing::info!(?action, "hotkey");
        let result = match action {
            Action::Open(name) => self.open(Some(name)),
            Action::Next => self.next(),
            Action::Snap => self.snap(),
        };
        if let Response::Error { message } = result {
            tracing::warn!(%message, "hotkey failed");
        }
    }

    fn inbox(&mut self, inbox: &Inbox) {
        while let Ok(item) = inbox.receiver.try_recv() {
            match item {
                Inbound::Hook(message) => self.hook(&message),
                Inbound::Control(request, reply) => {
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
            Request::Next => self.next(),
            Request::Demo => self.demo(),
            Request::Status => Response::Status(self.status()),
            Request::Reload => {
                self.load_config();
                self.hotkeys.register(&self.config);
                if let Some(b) = &self.borders {
                    let _ = b.set_config(BorderConfig::from(&self.config.border));
                }
                self.refresh_borders();
                match &self.config_error {
                    Some(e) => Response::error(format!("kept the previous config: {e}")),
                    None => Response::said("config reloaded"),
                }
            }
            Request::Stop => Response::said("stopping"),
        }
    }

    fn hook(&mut self, message: &HookMessage) {
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
        let before = self.agents.window_state(w);
        if self.agents.apply(w, &message.event) {
            let now = self.agents.window_state(w);
            self.note_states();
            if self.config.sound && now == Some(AgentState::Waiting) && before != now {
                access::chime();
            }
            tracing::debug!(window = format!("{w:#x}"), event = %message.event.hook_event_name, state = ?self.agents.window_state(w), "agent state");
            self.refresh_borders();
        }
    }

    /// Keeps `shown_since` in step with every window's state. A session that
    /// moves to another window changes the window it left as well.
    fn note_states(&mut self) {
        note_shown(
            &mut self.shown_since,
            self.agents.windows(),
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
        self.open_fleet(&fleet)
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
        let Some(fleet) = self.demo_fleet() else {
            return Response::error("cannot find daifuku.exe next to the daemon");
        };
        self.open_fleet(&fleet)
    }

    fn demo_fleet(&self) -> Option<daifuku_core::config::Fleet> {
        let exe = std::env::current_exe().ok()?.parent()?.join("daifuku.exe");
        Some(daifuku_core::config::Fleet {
            name: DEMO.to_owned(),
            count: 6,
            monitor: self
                .config
                .fleets
                .first()
                .map(|f| f.monitor.clone())
                .unwrap_or_default(),
            // The drive root: nothing personal in the path if a prompt shows.
            directory: Some(std::path::PathBuf::from(r"C:\")),
            command: Some(format!("& '{}' demo-agent {{n}}", exe.display())),
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
                && fleet::snap(&fleet, &self.config, record).is_ok()
            {
                snapped += record.windows().count();
            }
        }
        self.fleets.retain(|f| f.windows().next().is_some());
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
        let Some(index) = index else {
            return Response::said("that fleet is not open");
        };
        let record = self.fleets.remove(index);
        let mut closed = 0;
        for w in record.windows() {
            // Only a window that is still one of our terminals: a handle can
            // have been reused by anything since.
            if window::exists(w)
                && window::class(w) == daifuku_win::terminal::WINDOW_CLASS
                && window::close(w)
            {
                closed += 1;
            }
        }
        Response::said(format!(
            "closed {} of {}",
            fleet::terminals_count(closed),
            record.name
        ))
    }

    fn next(&mut self) -> Response {
        let queue: Vec<u64> = self
            .agents
            .needs_you()
            .into_iter()
            .filter(|&w| window::exists(w))
            .collect();
        let since = |w: u64| self.shown_since.get(&w).map(|&(_, t)| t);
        let Some(target) = next_target(&queue, window::foreground(), self.last_next, since) else {
            return Response::said("no agent is waiting");
        };
        if window::focus(target) {
            self.last_next = self.shown_since.get(&target).map(|&(_, t)| (target, t));
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
                for_seconds: self
                    .shown_since
                    .get(&w)
                    .map_or(0, |&(_, t)| t.elapsed().as_secs()),
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
            config.push_str(&format!(" (invalid, using the last good one: {e})"));
        }
        let mut hotkeys = self.hotkeys.describe();
        hotkeys.extend(
            self.hotkeys
                .refused
                .iter()
                .map(|k| format!("{k}: refused, another program holds it")),
        );
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            config,
            elevated: self.elevated,
            hotkeys,
            fleets,
            agents,
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
                Event::Destroyed(w) => {
                    let forgot = self.agents.forget_window(w);
                    self.shown_since.remove(&w);
                    for f in &mut self.fleets {
                        f.forget(w);
                    }
                    refresh |= forgot;
                }
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

    fn sweep(&mut self) {
        // The config changed on disk: take it, as `daifuku reload` would.
        if stamp(&self.config_path) != self.config_stamp {
            tracing::info!("config file changed, reloading");
            let _ = self.control(&Request::Reload);
        }
        // A monitor came, went or changed its size (a plug, a resolution, a
        // display link renegotiating): every open fleet goes back into the
        // grid of the monitor it belongs on now.
        let now = daifuku_win::monitor::monitors();
        if now != self.monitors {
            tracing::info!(
                before = self.monitors.len(),
                after = now.len(),
                "monitors changed, snapping fleets"
            );
            self.monitors = now;
            // A border's corners are worked out at the DPI of its screen, so
            // every border is handed over again, whether its window moved or
            // not; snapping does that pass itself.
            if let Some(b) = &self.borders {
                b.invalidate();
            }
            if self.fleets.is_empty() {
                self.refresh_borders();
            } else {
                let _ = self.snap();
            }
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
        }
        let dead: Vec<u64> = self
            .agents
            .windows()
            .into_keys()
            .filter(|&w| !window::exists(w))
            .collect();
        for w in &dead {
            self.agents.forget_window(*w);
            self.shown_since.remove(w);
        }
        for f in &mut self.fleets {
            f.prune();
        }
        if !dead.is_empty() {
            self.refresh_borders();
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
        let framed: Vec<(u64, AgentState, daifuku_core::Rect)> = self
            .agents
            .windows()
            .into_iter()
            .filter(|&(w, _)| {
                window::is_shown(w) && !window::is_minimised(w) && !window::is_maximised(w)
            })
            .filter_map(|(w, state)| window::frame(w).map(|rect| (w, state, rect)))
            .collect();
        self.update_pulse(
            framed
                .iter()
                .any(|&(_, state, _)| state == AgentState::Waiting),
        );
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
        let specs: Vec<BorderSpec> = framed
            .into_iter()
            .map(|(w, state, rect)| {
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
    /// away the contrast it is for.
    fn update_pulse(&mut self, waiting: bool) {
        let b = &self.config.border;
        let wanted = b.enabled && b.pulse && self.animations && !self.high_contrast && waiting;
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

/// Keeps `shown` in step with every window's state as of `now`: a window
/// whose state changed shows it since `now`, one whose state did not keeps
/// its time, and one that is gone is dropped.
fn note_shown(
    shown: &mut std::collections::HashMap<u64, (AgentState, std::time::Instant)>,
    states: std::collections::BTreeMap<u64, AgentState>,
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
