//! What Windows' accessibility settings ask of Daifuku.
//!
//! - **High contrast.** A person who turned it on chose their colours for a
//!   reason. Borders then take the theme's system colours instead of the
//!   palette's, and the per-state widths carry the rest.
//! - **Show animations off.** The waiting pulse is the only thing Daifuku
//!   animates; it stops.
//! - **Sound.** When asked for in the config, the system notification sound
//!   plays as an agent starts waiting, through the sound scheme the person
//!   has chosen, so it respects a muted or silent scheme.

use daifuku_core::config::{Colour, StateColours};
use windows::Win32::Graphics::Gdi::{
    COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HOTLIGHT, COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONASTERISK, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

/// Whether a high contrast theme is on.
#[must_use]
pub fn high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW {
        cbSize: u32::try_from(size_of::<HIGHCONTRASTW>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: hc is a HIGHCONTRASTW with its size set, as the call requires.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&raw mut hc).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && hc.dwFlags.contains(HCF_HIGHCONTRASTON)
}

/// Whether Windows is set to show animations. `true` when it cannot say.
#[must_use]
pub fn animations() -> bool {
    let mut on = windows::core::BOOL(1);
    // SAFETY: on is the BOOL the call writes.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on.as_bool()
}

fn system(index: SYS_COLOR_INDEX) -> Colour {
    // SAFETY: a plain query; the value is 0x00BBGGRR.
    let c = unsafe { GetSysColor(index) };
    let [r, g, b, _] = c.to_le_bytes();
    Colour::new(r, g, b)
}

/// The border colours for the current high contrast theme: its highlight for
/// waiting, the one state that asks for you; hyperlink for working; body text
/// for failed; disabled text for done.
#[must_use]
pub fn high_contrast_colours() -> StateColours {
    StateColours {
        working: system(COLOR_HOTLIGHT),
        waiting: system(COLOR_HIGHLIGHT),
        done: system(COLOR_GRAYTEXT),
        failed: system(COLOR_WINDOWTEXT),
    }
}

/// Plays the system's notification sound.
pub fn chime() {
    // SAFETY: no pointers; the sound plays asynchronously.
    let _ = unsafe { MessageBeep(MB_ICONASTERISK) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_answer_without_failing() {
        // Whatever this machine is set to, asking must work.
        let _ = high_contrast();
        let _ = animations();
        let c = high_contrast_colours();
        let _ = (c.working, c.waiting, c.done, c.failed);
    }
}
