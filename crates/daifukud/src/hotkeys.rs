//! Global hotkeys through `RegisterHotKey`.
//!
//! `RegisterHotKey` rather than a low-level keyboard hook: Windows delivers
//! the key to exactly one owner and swallows it, the daemon needs no hook DLL
//! and no input stream, and a combination another program already holds fails
//! to register instead of firing twice. That failure is logged and reported
//! by `daifuku status`, so a clash is visible rather than silent.

use daifuku_core::config::Config;
use daifuku_core::hotkey::Hotkey;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};

/// What a hotkey does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open or bring back a fleet.
    Open(String),
    /// Focus the terminal that has waited longest.
    Next,
    /// Put every fleet terminal back in its cell.
    Snap,
}

/// The registered hotkeys, by id.
#[derive(Default)]
pub struct Hotkeys {
    bound: Vec<(i32, Action, Hotkey)>,
    /// Combinations Windows refused, for `daifuku status`.
    pub refused: Vec<String>,
}

impl Hotkeys {
    /// Registers every hotkey in `config` for the calling thread, replacing
    /// any registered before.
    pub fn register(&mut self, config: &Config) {
        self.unregister();
        let mut wanted: Vec<(Action, Hotkey)> = Vec::new();
        for fleet in &config.fleets {
            if let Some(Ok(key)) = fleet
                .hotkey
                .as_ref()
                .map(daifuku_core::config::HotkeyText::parse)
            {
                wanted.push((Action::Open(fleet.name.clone()), key));
            }
        }
        if let Some(Ok(key)) = config
            .hotkeys
            .next_waiting
            .as_ref()
            .map(daifuku_core::config::HotkeyText::parse)
        {
            wanted.push((Action::Next, key));
        }
        if let Some(Ok(key)) = config
            .hotkeys
            .snap
            .as_ref()
            .map(daifuku_core::config::HotkeyText::parse)
        {
            wanted.push((Action::Snap, key));
        }
        for (index, (action, key)) in wanted.into_iter().enumerate() {
            let id = i32::try_from(index).unwrap_or(0) + 1;
            // SAFETY: registers for this thread's queue; no pointers.
            let ok = unsafe {
                RegisterHotKey(
                    None,
                    id,
                    HOT_KEY_MODIFIERS(key.modifiers) | MOD_NOREPEAT,
                    u32::from(key.vk),
                )
            };
            match ok {
                Ok(()) => {
                    tracing::info!(%key, ?action, "hotkey registered");
                    self.bound.push((id, action, key));
                }
                Err(error) => {
                    tracing::warn!(%key, ?action, %error, "hotkey refused, another program holds it");
                    self.refused.push(key.to_string());
                }
            }
        }
    }

    /// The action for a `WM_HOTKEY` id.
    #[must_use]
    pub fn action(&self, id: i32) -> Option<Action> {
        self.bound
            .iter()
            .find(|(i, _, _)| *i == id)
            .map(|(_, a, _)| a.clone())
    }

    /// Every bound combination and what it does, for `daifuku status`.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        self.bound
            .iter()
            .map(|(_, action, key)| match action {
                Action::Open(name) => format!("{key}: open {name}"),
                Action::Next => format!("{key}: next waiting agent"),
                Action::Snap => format!("{key}: snap fleets back"),
            })
            .collect()
    }

    /// Releases every hotkey.
    pub fn unregister(&mut self) {
        for (id, _, _) in self.bound.drain(..) {
            // SAFETY: an id this thread registered.
            let _ = unsafe { UnregisterHotKey(None, id) };
        }
        self.refused.clear();
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.unregister();
    }
}
