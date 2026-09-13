//! The system tray icon and its menu.
//!
//! **No tests live in this file, and none should.** It is reachable only from
//! `lib.rs`'s `run()`, which is dead code in a `cargo test` build and gets
//! stripped — that is what keeps Tauri's Wry window machinery, and with it the
//! whole Win32 GUI import stack, out of the test binary. A `cargo test` binary
//! carries no application manifest, so Windows resolves `comctl32.dll` to the
//! v5 side-by-side assembly and the binary dies at load with
//! `STATUS_ENTRYPOINT_NOT_FOUND` before running anything. See
//! `state_machine::supervisor::on_library_changed`, which documents the
//! incident. Anything here worth testing — `CloseAction` parsing — belongs in
//! `core`, which names no `tauri` type.
//!
//! The menu is deliberately three items. A "Start/Stop recording" entry was
//! considered and rejected: `start_recording` races the state machine, which
//! doesn't know about the call (`src/dev/registry.ts` says so, and
//! `Supervisor::start_recording` spells out the divergence), so promoting it
//! from a dev affordance to a shipped one would ship a known bug.

use crate::{error, info, warn};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

const MENU_OPEN: &str = "tray-open";
const MENU_SETTINGS: &str = "tray-settings";
const MENU_QUIT: &str = "tray-quit";

/// Emitted to the frontend to ask it to switch views. Carries the view name.
/// The window may have just been created, in which case the frontend isn't
/// listening yet — `show_window` handles that by passing the view through the
/// URL instead.
pub(crate) const NAVIGATE_EVENT: &str = "navigate";

pub(crate) fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "Open ninja-recorder", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, MENU_SETTINGS, "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("ninja-recorder")
        .menu(&menu)
        // Left-click opens the window; the menu is the right-click gesture,
        // which is what every other tray app on Windows does.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => show_window(app, None),
            MENU_SETTINGS => show_window(app, Some("settings")),
            MENU_QUIT => request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle(), None);
            }
        });

    // Reuses the icon already decoded from the bundle, so no `image-png` /
    // `image-ico` Cargo feature is needed. Without an icon the tray would be
    // an invisible click target, so skip it rather than ship that.
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
        tray.build(app)?;
    } else {
        warn!("tray", "no bundled window icon, skipping the tray icon");
    }
    Ok(())
}

/// Brings the UI up, creating the window if it isn't there.
///
/// Both cases are real: `--hidden` starts with no window at all, and
/// `CloseAction::CloseWindow` destroys it. `view` routes the frontend once
/// it's up.
fn show_window(app: &AppHandle, view: Option<&str>) {
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
