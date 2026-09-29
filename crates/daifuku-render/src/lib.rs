//! Status borders: one layered, click-through frame per agent terminal, in the
//! colour of what its agent is doing.
//!
//! The renderer is Mochi's border renderer, adapted: Mochi colours a border by
//! the kind of container it frames, Daifuku by agent state, so a
//! [`BorderSpec`] carries its [`Colour`] directly. Everything else is as proven
//! there: one thread owns every frame, the daemon only ever sends the desired
//! end state, a frame whose pixels did not change is moved rather than
//! repainted, and each frame sits directly above its window in the z-order
//! instead of on top of everything.
//!
//! Daifuku runs elevated, which is what lets it draw around administrator
//! terminals at all: Windows refuses to stack a frame from an ordinary process
//! against an elevated window.

#![warn(missing_docs)]

pub mod border;
pub mod color;
pub mod geometry;

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use border::BorderManager;
pub use border::{BorderChanges, BorderConfig, BorderDiff, BorderSpec};
pub use color::ColourExt;
pub use daifuku_core::Rect;
pub use daifuku_core::config::Colour;

/// A window handle, stored as the raw `HWND` value.
///
/// `HWND` is a raw pointer and so neither `Send` nor `Sync`, which would stop
/// the daemon from putting one in a channel message. This is the same number
/// with the thread-safety markers a plain integer has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WindowHandle(pub isize);

impl WindowHandle {
    /// The null handle: "no window".
    pub const NONE: Self = Self(0);

    /// `true` for the null handle.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// The raw value, for logging.
    #[must_use]
    pub const fn raw(self) -> isize {
        self.0
    }

    /// From the `u64` form handles travel in between Daifuku's crates.
    #[must_use]
    pub fn from_raw(raw: u64) -> Self {
        Self(isize::try_from(raw).unwrap_or(0))
    }
}

impl std::fmt::Display for WindowHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

#[cfg(windows)]
impl WindowHandle {
    /// Borrows the handle as an `HWND`.
    #[must_use]
    pub fn hwnd(self) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND(self.0 as *mut core::ffi::c_void)
    }
}

#[cfg(windows)]
impl From<windows::Win32::Foundation::HWND> for WindowHandle {
    fn from(hwnd: windows::Win32::Foundation::HWND) -> Self {
        Self(hwnd.0 as isize)
    }
}

#[cfg(windows)]
impl From<WindowHandle> for windows::Win32::Foundation::HWND {
    fn from(handle: WindowHandle) -> Self {
        handle.hwnd()
    }
}

/// Where one window is now: what [`BorderManager::follow_frame`] moves a
/// border to when its window moved and nothing else changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameUpdate {
    /// The window that moved.
    pub handle: WindowHandle,
    /// Its visible frame now.
    pub rect: Rect,
    /// `true` when this is where the window will stay, not a step of a drag.
    pub finished: bool,
}

/// Everything that can go wrong while drawing.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// A Win32 or COM call failed.
    #[cfg(windows)]
    #[error("win32 call failed: {0}")]
    Win32(#[from] windows::core::Error),

    /// The worker thread behind a manager handle has gone away.
    #[error("the {0} thread is not running")]
    ThreadGone(&'static str),

    /// A worker thread could not be started.
    #[error("could not start the {0} thread: {1}")]
    ThreadStart(&'static str, String),
}

/// The crate result type.
pub type Result<T> = std::result::Result<T, RenderError>;

impl RenderError {
    /// True when Windows turned the call down rather than failing it.
    ///
    /// UIPI refuses every window call against a window of a higher integrity,
    /// permanently: there is nothing to retry. The border thread uses this to
    /// tell "try again later" from "never again", so one such window does not
    /// cost a failed call and a log line on every pass.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        #[cfg(windows)]
        {
            matches!(
                self,
                Self::Win32(e) if e.code() == windows::Win32::Foundation::E_ACCESSDENIED
            )
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG};

    #[test]
    fn only_access_denied_counts_as_a_refusal() {
        assert!(
            RenderError::Win32(windows::core::Error::from_hresult(E_ACCESSDENIED)).is_refusal()
        );
        assert!(!RenderError::Win32(windows::core::Error::from_hresult(E_INVALIDARG)).is_refusal());
        assert!(!RenderError::ThreadGone("border").is_refusal());
        assert!(!RenderError::ThreadStart("border", "nope".into()).is_refusal());
    }

    #[test]
    fn handles_survive_the_trip_through_u64() {
        let h = WindowHandle::from_raw(0x00b8_089e);
        assert_eq!(h.raw(), 0x00b8_089e);
        assert_eq!(h.to_string(), "0xb8089e");
        assert!(WindowHandle::from_raw(0).is_none());
    }
}
