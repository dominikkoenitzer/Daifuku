//! The attached monitors, in physical pixels.
//!
//! Call [`crate::dpi::per_monitor_v2`] first. Before it, every rectangle here
//! is scaled to the primary monitor's DPI and wrong for every other one.

use daifuku_core::Rect;
use daifuku_core::monitor::MonitorInfo;
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};

use crate::wide::from_wide;

/// Every monitor, in the order Windows lists them.
#[must_use]
pub fn monitors() -> Vec<MonitorInfo> {
    let mut handles: Vec<HMONITOR> = Vec::with_capacity(4);
    // SAFETY: the callback only runs during this call, and lparam points at
    // `handles`, which outlives it.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&raw mut handles as isize));
    }
    handles.into_iter().filter_map(info).collect()
}

/// Where the mouse is, or the origin if Windows will not say (a locked
/// session, a secure desktop).
#[must_use]
pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: p is a valid out pointer.
    if unsafe { GetCursorPos(&raw mut p) }.is_ok() {
        (p.x, p.y)
    } else {
        (0, 0)
    }
}

unsafe extern "system" fn collect(
    monitor: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> windows::core::BOOL {
    // SAFETY: lparam is the Vec handed to EnumDisplayMonitors above.
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HMONITOR>) };
    list.push(monitor);
    true.into()
}

pub(crate) const fn rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

fn info(monitor: HMONITOR) -> Option<MonitorInfo> {
    let mut mi = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: u32::try_from(size_of::<MONITORINFOEXW>()).unwrap_or(0),
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: mi is a MONITORINFOEXW with cbSize set, which the call accepts
    // through a MONITORINFO pointer.
    let ok = unsafe { GetMonitorInfoW(monitor, (&raw mut mi).cast::<MONITORINFO>()) };
    if !ok.as_bool() {
        return None;
    }
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // SAFETY: both are valid out pointers; on failure they keep 96.
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) };
    Some(MonitorInfo {
        device: from_wide(&mi.szDevice),
        bounds: rect(mi.monitorInfo.rcMonitor),
        work: rect(mi.monitorInfo.rcWork),
        dpi: dpi_x,
        primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_has_one_primary_monitor() {
        crate::dpi::per_monitor_v2();
        let all = monitors();
        // A CI runner has a desktop too, so there is always at least one.
        if all.is_empty() {
            return;
        }
        assert_eq!(all.iter().filter(|m| m.primary).count(), 1);
        for m in &all {
            assert!(!m.bounds.is_empty(), "{m:?}");
            assert!(m.work.width() <= m.bounds.width() && m.work.height() <= m.bounds.height());
            assert!(m.dpi >= 96, "{m:?}");
        }
    }
}
