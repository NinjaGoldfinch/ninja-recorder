//! Locating the League game window and reading its client-area size, so the
//! capture resolution can follow the actual window instead of a hardcoded
//! guess (DEVELOPMENT.md §2.4: "resolution follows the game window").
//!
//! These identifiers (title/class/process) are the same ones the reference
//! implementation uses — see DEVELOPMENT.md §2.1 — and are stable across
//! League's client versions; only the title is locale-dependent, which is
//! why `libobs_recorder::settings::Window` matching in `mod.rs` is
//! configured to prioritize the process name over the title.

use libobs_recorder::settings::Resolution;
use windows::core::PCSTR;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowA, GetClientRect};

pub const WINDOW_TITLE: &str = "League of Legends (TM) Client";
pub const WINDOW_CLASS: &str = "RiotWindowClass";
pub const WINDOW_PROCESS: &str = "League of Legends.exe";

/// Finds the running League game window, if any. `None` doesn't
/// necessarily mean the game isn't running — the window can take a moment
/// to appear after Live Client Data starts responding, which is what
/// triggers `Recorder::start` (DEVELOPMENT.md §3.4).
pub fn find_window() -> Option<HWND> {
    // Win32 title/class strings are ANSI (`FindWindowA`) and must be
    // null-terminated; `PCSTR` borrows these buffers so they need to
    // outlive the call.
    let mut title = WINDOW_TITLE.to_owned();
    title.push('\0');
    let mut class = WINDOW_CLASS.to_owned();
    class.push('\0');

    let class_ptr = PCSTR(class.as_ptr());
    let title_ptr = PCSTR(title.as_ptr());

    unsafe { FindWindowA(class_ptr, title_ptr) }.ok()
}

/// Client-area size of `hwnd`, or `None` while it isn't meaningful yet.
///
/// Immediately after the window is created, Windows briefly reports a
/// (1, 1) client rect under per-monitor DPI awareness (needed to get the
/// real, correctly-scaled size on HiDPI displays) — treat that as "not
/// ready" rather than a real 1x1 window.
pub fn window_size(hwnd: HWND) -> Option<Resolution> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) }.ok()?;

    if rect.right > 1 && rect.bottom > 1 {
        Some(Resolution::new(rect.right as u32, rect.bottom as u32))
    } else {
        None
    }
}
