//! What the window still needs from what used to be the tray.
//!
//! **The tray icon is the daemon's** (§3.1, WS3.3). It lives in
//! `daemon::pump`, on that process's own Win32 message loop, because the
//! process that must outlive the window is the one worth reaching. This file is
//! what stayed behind: showing the window, and the close button's Quit.
//!
//! The name is now a little wrong and is kept anyway, because the two functions
//! in it are still the tray's requests — they just arrive from another process
//! as an `Event::ShowUi` off the pipe rather than from a menu callback in this
//! one.
//!
//! **No tests live in this file, and none should.** It is reachable only from
//! `lib.rs`'s `run()` and `ui::link`, which are dead code in a `cargo test`
//! build and get stripped — that is what keeps Tauri's Wry window machinery,
//! and with it the whole Win32 GUI import stack, out of the test binary. A
//! `cargo test` binary carries no application manifest, so Windows resolves
//! `comctl32.dll` to the v5 side-by-side assembly and the binary dies at load
//! with `STATUS_ENTRYPOINT_NOT_FOUND` before running anything. See
//! `state_machine::supervisor::on_library_changed`, which documents the
//! incident. Anything here worth testing — `CloseAction` parsing — belongs in
//! `core`, which names no `tauri` type.

use crate::{error, info};
use tauri::{AppHandle, Manager};

/// Emitted to the frontend to ask it to switch views. Carries the view name.
///
/// The window may have just been created, in which case the frontend is not
/// listening yet — `show_window` handles that by passing the view through the
/// URL instead.
pub(crate) const NAVIGATE_EVENT: &str = "navigate";

/// Brings the UI up, creating the window if it isn't there.
///
/// Both cases are real: `--hidden` starts with no window at all, and
/// `CloseAction::CloseWindow` destroys it. `view` routes the frontend once
/// it's up.
///
/// `pub(crate)` since WS3.3, because the tray that calls it is in the *daemon*
/// now. Its Open and Settings items arrive here as an `Event::ShowUi` off the
/// pipe (`ui::link`), which is the same request this has always answered,
/// asked from another process.
pub(crate) fn show_window(app: &AppHandle, view: Option<&str>) {
    if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(view) = view {
            // Already loaded, so ask it to navigate.
            use tauri::Emitter;
            let _ = window.emit(NAVIGATE_EVENT, view);
        }
        return;
    }

    // No window yet. The frontend cannot be listening for an event it will
    // only subscribe to after loading, so the requested view rides in on the
    // URL fragment and `router.ts` reads it at startup.
    let app = app.clone();
    let view = view.map(str::to_owned);
    // Spawned rather than called inline: this runs on the main thread from a
    // tray callback, and `dev_open_portal` documents what building a window
    // re-entrantly from a callback does — a window with no webview attached.
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::create_main_window(&app, view.as_deref()) {
            error!("tray", "could not open the window: {e}");
        }
    });
}

/// Quits, finalizing an in-flight recording first so the game isn't lost.
///
/// The finalize runs on a blocking thread, never here: menu handlers run on
/// the main thread, and `Supervisor::finalize_for_shutdown` does an ffmpeg
/// remux and a retention sweep. Doing that inline would freeze the tray, the
/// menu and every window for seconds.
pub(crate) fn request_quit(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let supervisor = {
            let state = app.state::<crate::AppState>();
            std::sync::Arc::clone(&state.supervisor)
        };
        if supervisor.finalize_for_shutdown() {
            info!("tray", "finalized an in-flight recording before quitting");
        }
        // `exit` rather than letting the last window close: this is the
        // programmatic path, which `RunEvent::ExitRequested` sees as
        // `code: Some(_)` and therefore does not veto.
        app.exit(0);
    });
}
