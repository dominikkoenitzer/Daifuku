//! The daemon's state and its message loop.

use std::path::PathBuf;

use anyhow::Context;
use daifuku_core::config::Config;
use daifuku_core::protocol::{AgentWindow, FleetStatus, HookMessage, Request, Response, Status};
use daifuku_core::state::Agents;
use daifuku_render::{BorderConfig, BorderManager, BorderSpec, WindowHandle};
use daifuku_win::{dpi, paths, process, window};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GA_ROOT, GetAncestor, GetMessageW, KillTimer, MSG, PostQuitMessage, SetTimer,
    TranslateMessage, WM_HOTKEY, WM_TIMER,
};

use crate::events::{self, Event};
use crate::fleet::{self, OpenFleet};
use crate::hotkeys::{Action, Hotkeys};
use crate::ipc::{self, Inbound, Inbox, WM_INBOX};
use crate::logging;

/// The most sessions tracked at once. A hook can report any session id it
/// likes; past this, new ones are ignored instead of growing the map forever.
const MAX_SESSIONS: usize = 512;

/// How often dead windows are swept, in milliseconds. Destroy events do most
/// of the work; this catches the ones Windows did not deliver.
const SWEEP_MS: u32 = 2000;

struct Daemon {
    config: Config,
    config_path: PathBuf,
    config_error: Option<String>,
    agents: Agents<u64>,
    fleets: Vec<OpenFleet>,
    borders: Option<BorderManager>,
    hotkeys: Hotkeys,
    elevated: bool,
}

/// Runs the daemon until it is told to stop.
pub fn run(config_override: Option<PathBuf>) -> anyhow::Result<()> {
    dpi::per_monitor_v2();
    let elevated = process::current_is_elevated();
    let _log = logging::init(paths::log_dir(elevated).as_deref());
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        elevated,
        "daifukud starting"
    );

    // SAFETY: no arguments.
    let main_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    let inbox = ipc::start(main_thread).context("another daifukud already runs in this session")?;

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
    };
    d.load_config();
    d.borders = BorderManager::new(BorderConfig::from(&d.config.border))
        .inspect_err(|e| tracing::error!(error = %e, "no borders"))
        .ok();
    d.hotkeys.register(&d.config);
    let _hooks = events::Hooks::install();
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
            WM_TIMER if msg.hwnd.is_invalid() => d.sweep(),
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
    tracing::info!("daifukud stopped");
    Ok(())
}

impl Daemon {
    fn load_config(&mut self) {
        let text = match std::fs::read_to_string(&self.config_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                self.config_error = Some(format!("{}: {e}", self.config_path.display()));
                tracing::error!(path = %self.config_path.display(), error = %e, "config unreadable, keeping the last good one");
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
        let w = message.window;
        // Only a live top-level window: anything else in a hook message is a
        // stale handle or a made-up one, and must not get a frame.
        // SAFETY: GetAncestor accepts any handle.
        let is_top = window::exists(w)
            && daifuku_win::raw(unsafe { GetAncestor(daifuku_win::hwnd(w), GA_ROOT) }) == w;
        if !is_top {
            return;
        }
        if self.agents.len() >= MAX_SESSIONS && self.agents.window_state(w).is_none() {
            return;
        }
        if self.agents.apply(w, &message.event) {
            tracing::debug!(window = format!("{w:#x}"), event = %message.event.hook_event_name, state = ?self.agents.window_state(w), "agent state");
            self.refresh_borders();
        }
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
        let index = self
            .fleets
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&fleet.name));
        let current = index.map(|i| self.fleets.remove(i));
        match fleet::open(&fleet, &self.config, current, self.elevated) {
            Ok((record, message)) => {
                tracing::info!(%message);
                self.fleets.push(record);
                self.refresh_borders();
                Response::said(message)
            }
            Err(e) => Response::error(format!("{e:#}")),
        }
    }

    fn snap(&mut self) -> Response {
        let mut snapped = 0;
        for record in &mut self.fleets {
            if let Some(fleet) = self.config.fleet(&record.name)
                && fleet::snap(fleet, &self.config, record).is_ok()
            {
                snapped += record.windows.len();
            }
        }
        self.fleets.retain(|f| !f.windows.is_empty());
        self.refresh_borders();
        Response::said(format!("snapped {snapped} terminals"))
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
        for w in record.windows {
            // Only a window that is still one of our terminals: a handle can
            // have been reused by anything since.
            if window::exists(w)
                && window::class(w) == daifuku_win::terminal::WINDOW_CLASS
                && window::close(w)
            {
                closed += 1;
            }
        }
        Response::said(format!("closed {closed} terminals of {}", record.name))
    }

    fn next(&mut self) -> Response {
        let queue: Vec<u64> = self
            .agents
            .needs_you()
            .into_iter()
            .filter(|&w| window::exists(w))
            .collect();
        let Some(&first) = queue.first() else {
            return Response::said("no agent is waiting");
        };
        // Pressing it again while on the first one moves on to the second.
        let target = if window::foreground() == first {
            queue.get(1).copied().unwrap_or(first)
        } else {
            first
        };
        if window::focus(target) {
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
            })
            .collect();
        let fleets = self
            .fleets
            .iter()
            .map(|f| FleetStatus {
                name: f.name.clone(),
                monitor: f.monitor.clone(),
                windows: f.windows.clone(),
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
        for e in events {
            match e {
                Event::Destroyed(w) => {
                    let forgot = self.agents.forget_window(w);
                    for f in &mut self.fleets {
                        f.windows.retain(|&x| x != w);
                    }
                    refresh |= forgot;
                }
                Event::Moved(w) | Event::Visibility(w) => {
                    refresh |= self.agents.window_state(w).is_some();
                }
            }
        }
        if refresh {
            self.refresh_borders();
        }
    }

    fn sweep(&mut self) {
        let dead: Vec<u64> = self
            .agents
            .windows()
            .into_keys()
            .filter(|&w| !window::exists(w))
            .collect();
        for w in &dead {
            self.agents.forget_window(*w);
        }
        for f in &mut self.fleets {
            f.prune();
        }
        if !dead.is_empty() {
            self.refresh_borders();
        }
    }

    /// Sends the borders their end state: one frame per agent window that is
    /// on screen, in its state's colour.
    fn refresh_borders(&self) {
        let Some(borders) = &self.borders else { return };
        let colours = self.config.border.colours;
        let specs: Vec<BorderSpec> = self
            .agents
            .windows()
            .into_iter()
            .filter(|&(w, _)| window::is_shown(w) && !window::is_minimised(w))
            .filter_map(|(w, state)| {
                window::frame(w)
                    .map(|rect| BorderSpec::new(WindowHandle::from_raw(w), rect, colours.of(state)))
            })
            .collect();
        if let Err(e) = borders.update(None, specs) {
            tracing::warn!(error = %e, "could not update borders");
        }
    }
}
