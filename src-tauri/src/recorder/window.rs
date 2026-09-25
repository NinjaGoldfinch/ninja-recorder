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
use windows::Win32::UI::WindowsAndMessaging::{FindWindowA, FindWindowW, GetClientRect, IsIconic};
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

/// The width and height of `hwnd`'s client area in pixels, or `None` while
/// there is nothing there worth capturing: the window is minimised, the call
/// failed, or either side is shorter than `MIN_SIDE`. Both callers poll
/// this until it answers, so `None` means "not yet", never "give up".
///
/// - **Minimised.** Checked first, with [`IsIconic`][isiconic]. A minimised
///   window has no client area on screen, and Windows.Graphics.Capture sends
///   no frames for one (`own/win/session.rs`), so there is no size to report
///   whatever the rectangle says.
/// - **Failed call.** [`GetClientRect`][getclientrect] returns zero on
///   failure, which includes a handle that no longer names a window: the game
///   can close between `find_by_class` and this call. That is `None`. There is
///   no separate [`IsWindow`][iswindow] check, because its page warns against
///   using it on a window another thread created (the handle can be destroyed,
///   or recycled, straight after), and `GetClientRect` failing already says
///   the same thing without the race.
/// - **Degenerate.** Microsoft documents the rectangle's `left` and `top` as
///   zero and `right` and `bottom` as the width and height, with the bottom
///   right exclusive. `usable_size` still subtracts rather than trusting the
///   zeros, and rejects a negative, empty or sub-`MIN_SIDE` result.
///
/// The size is not rounded to even here. The own backend sizes its encoder
/// from what WGC reports, not from this, and evens that itself
/// (`status::even_size`); this call only tells it the window is ready.
///
/// **The units depend on the caller's DPI awareness, not the game's.**
/// Microsoft's [high-DPI guide][hidpi] says a thread that is DPI unaware or
/// system aware may be handed values Windows has scaled into its own
/// coordinate space, and that which APIs do so "is not currently sufficiently
/// documented". The executable's manifest (`tauri-build`'s default) declares
/// no DPI awareness, which [leaves the process default unaware][default]. The
/// UI turns per-monitor aware at runtime when Tauri's windowing library starts
/// its event loop, but the daemon, where the recorder runs, builds no Tauri
/// app and sets nothing. So on a display scaled above 100% this can be smaller
/// than the game's real pixel size. The own backend only asks whether a size
/// exists, so that does not reach it. The libobs backend uses the value as its
/// output resolution, and #51 deletes that caller.
///
/// [getclientrect]: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclientrect
/// [isiconic]: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-isiconic
/// [iswindow]: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-iswindow
/// [hidpi]: https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows
/// [default]: https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process
pub fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    // Written from Microsoft's documentation (see links), not from any other implementation. docs/licensing.md, #50.

    // SAFETY: `IsIconic` only reads window state and takes the handle by
    // value; a handle that names no window is answered, not dereferenced.
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return None;
    }

    let mut rect = RECT::default();
    // SAFETY: `rect` is a live, writable `RECT` for the length of the call,
    // which is all `lpRect` asks for. A stale handle makes the call fail,
    // which is an `Err` here, not undefined behaviour.
    unsafe { GetClientRect(hwnd, &mut rect) }.ok()?;
    usable_size(&rect)
}

/// The shortest side a client area can have and still be a picture. Both
/// backends encode 4:2:0 video, which stores colour once per 2x2 block of
/// pixels, so a side shorter than two pixels has no frame to give an encoder:
/// the own backend's evening would round it down to zero.
const MIN_SIDE: u32 = 2;

/// A client rectangle as `(width, height)`, or `None` if either side is
/// negative, overflows, or is shorter than `MIN_SIDE`. Pure, so the rules
/// are tested without a window.
fn usable_size(rect: &RECT) -> Option<(u32, u32)> {
    let side = |low: i32, high: i32| {
        let length = u32::try_from(high.checked_sub(low)?).ok()?;
        (length >= MIN_SIDE).then_some(length)
    };
    Some((side(rect.left, rect.right)?, side(rect.top, rect.bottom)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn a_normal_client_area_is_its_width_and_height() {
        assert_eq!(usable_size(&rect(0, 0, 1920, 1080)), Some((1920, 1080)));
    }

    #[test]
    fn odd_sizes_are_reported_as_they_are() {
        // Evening is the caller's job, and only the own backend does it.
        assert_eq!(usable_size(&rect(0, 0, 1279, 719)), Some((1279, 719)));
    }

    #[test]
    fn a_nonzero_origin_is_subtracted() {
        assert_eq!(usable_size(&rect(10, 20, 810, 620)), Some((800, 600)));
    }

    #[test]
    fn an_empty_rectangle_is_none() {
        assert_eq!(usable_size(&rect(0, 0, 0, 0)), None);
        assert_eq!(usable_size(&rect(0, 0, 1920, 0)), None);
        assert_eq!(usable_size(&rect(0, 0, 0, 1080)), None);
    }

    #[test]
    fn a_side_shorter_than_min_side_is_none() {
        assert_eq!(usable_size(&rect(0, 0, 1, 1)), None);
        assert_eq!(usable_size(&rect(0, 0, 1920, 1)), None);
        assert_eq!(usable_size(&rect(0, 0, 1, 1080)), None);
        assert_eq!(usable_size(&rect(0, 0, 2, 2)), Some((2, 2)));
    }

    #[test]
    fn an_inverted_rectangle_is_none() {
        assert_eq!(usable_size(&rect(0, 0, -5, 1080)), None);
        assert_eq!(usable_size(&rect(100, 0, 50, 1080)), None);
    }

    #[test]
    fn a_span_that_overflows_i32_is_none_rather_than_a_panic() {
        assert_eq!(usable_size(&rect(i32::MIN, 0, i32::MAX, 1080)), None);
    }
}
