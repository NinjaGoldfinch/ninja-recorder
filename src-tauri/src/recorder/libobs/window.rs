//! The game window, in the libobs backend's vocabulary. The lookup itself is
//! `recorder::window`, shared with the own backend; this only turns its size
//! into the fork's `Resolution`, which must not leak out of `libobs/`.

use libobs_recorder::settings::Resolution;
use windows::Win32::Foundation::HWND;

pub use crate::recorder::window::{WINDOW_CLASS, WINDOW_PROCESS, WINDOW_TITLE, find_window};

/// Client-area size of `hwnd` as a libobs `Resolution`, or `None` while it
/// isn't meaningful yet (see `recorder::window::client_size`).
pub fn window_size(hwnd: HWND) -> Option<Resolution> {
    crate::recorder::window::client_size(hwnd).map(|(w, h)| Resolution::new(w, h))
}
