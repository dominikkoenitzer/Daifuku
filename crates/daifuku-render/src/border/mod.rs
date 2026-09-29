//! Borders: one layered frame window per agent terminal.
//!
//! The daemon does not drive individual frames. It sends the whole desired end
//! state to [`BorderManager::update`] whenever a state or a position changes,
//! and the manager works out which frames to create, move, recolour or take
//! down. Everything it does not mention is left exactly as it is.
//!
//! ```no_run
//! use daifuku_render::{BorderConfig, BorderManager, BorderSpec, Colour, Rect, WindowHandle};
//!
//! # fn main() -> daifuku_render::Result<()> {
//! let borders = BorderManager::new(BorderConfig::default())?;
//! borders.update(
//!     None,
//!     vec![BorderSpec::new(
//!         WindowHandle(0x1234),
//!         Rect::new(100, 100, 900, 700),
//!         Colour::new(0xf9, 0xe2, 0xaf),
//!     )],
//! )?;
//! # Ok(())
//! # }
//! ```

use daifuku_core::config::{Border, Colour};

use crate::{Rect, WindowHandle};

mod diff;
#[cfg(windows)]
mod manager;
#[cfg(windows)]
mod window;

pub use diff::{BorderChanges, BorderDiff};
#[cfg(windows)]
pub use manager::BorderManager;
#[cfg(windows)]
pub use window::BorderWindow;

/// How the frames look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderConfig {
    /// Draw frames at all.
    pub enabled: bool,
    /// Thickness in physical pixels. Not scaled by DPI: a 4 px frame is 4 px
    /// on a 4K monitor and on a 1080p one.
    pub width: i32,
    /// How far the frame sits outside the window's visible edge, on top of
    /// the width. Negative laps it over the window's own edge.
    pub offset: i32,
    /// Rounded corners, matching the window's own. Windows 11 rounds its
    /// windows, Windows 10 does not.
    pub rounded: bool,
}

impl Default for BorderConfig {
    fn default() -> Self {
        Self::from(&Border::default())
    }
}

impl From<&Border> for BorderConfig {
    /// The config file's `border` section. The corner shape is not a setting:
    /// it follows the operating system, because a rounded frame around a
    /// square window looks broken.
    fn from(border: &Border) -> Self {
        Self {
            enabled: border.enabled,
            width: border.width,
            offset: border.offset,
            #[cfg(windows)]
            rounded: crate::win::os_rounds_corners(),
            #[cfg(not(windows))]
            rounded: true,
        }
    }
}

/// One frame the daemon wants on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderSpec {
    /// The window the frame belongs to. It is the z-order anchor and the
    /// identity of the frame; the renderer never changes it.
    pub target: WindowHandle,
    /// The window's visible frame, in physical screen pixels.
    pub rect: Rect,
    /// What to paint it.
    pub colour: Colour,
}

impl BorderSpec {
    /// A spec from its parts.
    #[must_use]
    pub const fn new(target: WindowHandle, rect: Rect, colour: Colour) -> Self {
        Self {
            target,
            rect,
            colour,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_file_section_drives_the_frames() {
        let border = Border {
            enabled: false,
            width: 7,
            offset: -2,
            ..Border::default()
        };
        let config = BorderConfig::from(&border);
        assert!(!config.enabled);
        assert_eq!((config.width, config.offset), (7, -2));
    }

    #[test]
    fn the_default_is_the_config_files_default() {
        let config = BorderConfig::default();
        assert!(config.enabled);
        assert_eq!(config.width, Border::default().width);
    }
}
