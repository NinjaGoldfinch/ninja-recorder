//! Locating the League game window and reading its client-area size, for
//! both Windows backends: libobs names the window to its worker, and the own
//! backend hands it to Windows.Graphics.Capture. One copy, so the two cannot
//! disagree about which window the game is (DEVELOPMENT.md §2.4: "resolution
//! follows the game window").
//!
//! These identifiers (title/class/process) are the same ones the reference
//! implementation uses — see DEVELOPMENT.md §2.1 — and are stable across
//! League's client versions; only the title is locale-dependent, which is
//! why the libobs backend's `Window` matching is configured to prioritize the
//! process name over the title, and why the own backend looks the window up
//! by class alone.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowA, FindWindowW, GetClientRect};
use windows::core::{PCSTR, w};

pub const WINDOW_TITLE: &str = "League of Legends (TM) Client";
pub const WINDOW_CLASS: &str = "RiotWindowClass";
pub const WINDOW_PROCESS: &str = "League of Legends.exe";

/// Finds the running League game window by class and title, if any. `None`
/// doesn't necessarily mean the game isn't running — the window can take a
/// moment to appear after Live Client Data starts responding, which is what
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

    // SAFETY: both pointers are to NUL-terminated buffers that outlive the
    // call; a missing window is an error result, not undefined behaviour.
    unsafe { FindWindowA(class_ptr, title_ptr) }.ok()
}

/// The game window by class alone. The title is translated in some locales
/// and the class is not, so this is the lookup that finds it everywhere.
pub fn find_by_class() -> Option<HWND> {
    // SAFETY: a static, NUL-terminated class name and a null title.
    unsafe { FindWindowW(w!("RiotWindowClass"), None) }.ok()
}

/// Client-area size of `hwnd`, or `None` while it isn't meaningful yet.
///
/// Immediately after the window is created, Windows briefly reports a
/// (1, 1) client rect under per-monitor DPI awareness (needed to get the
/// real, correctly-scaled size on HiDPI displays) — treat that as "not
/// ready" rather than a real 1x1 window. A minimised window reports (0, 0),
/// which is not ready either.
pub fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a live out-parameter; a stale handle is an error
    // result rather than undefined behaviour.
    unsafe { GetClientRect(hwnd, &mut rect) }.ok()?;

    if rect.right > 1 && rect.bottom > 1 {
        Some((rect.right as u32, rect.bottom as u32))
    } else {
        None
    }
}
