//! Dev portal backend (`dev.html`). Everything under this module is
//! compiled only with the `devtools` Cargo feature, which is off by
//! default — see `Cargo.toml`. That matters: these commands execute
//! arbitrary SQL, write rows straight past the typed `db` API, delete
//! files, and drive the state machine, none of which belongs in a shipped
//! installer.
//!
//! The portal exists because the backend has outgrown what the app's UI
//! can reach. Most of `state_machine::supervisor`, all of the marker and
//! sample pipeline, and every retention path either need a live League
//! client or need a library that only a real recording session produces.
//! DEVELOPMENT.md §3.3 asked for a fixture replay mode; this is it.

mod fixtures_api;
mod info;
mod log_api;
mod recording_actions;
mod recording_api;
mod retention_api;
mod seed;
mod simulate;
mod sql;
mod trim;

// Glob re-exports, not a named list: `#[tauri::command]` expands to the
// function *plus* hidden `__cmd__*` / `__tauri_command_name_*` items that
// `generate_handler!` resolves through the same path, and naming only the
// function leaves those behind in the submodule.
pub use fixtures_api::*;
pub use info::*;
pub use log_api::*;
pub use recording_actions::*;
pub use recording_api::*;
pub use retention_api::*;
pub use seed::*;
pub use simulate::*;
pub use sql::*;
pub use trim::*;

use std::sync::Mutex;

/// Portal-owned state, managed alongside `AppState`. Kept separate so
/// nothing in the production `AppState` has to know the portal exists.
#[derive(Default)]
pub struct DevState {
    pub(crate) replay: Mutex<Option<simulate::ReplayHandle>>,
}

/// The window label the portal runs in. Matches
/// `capabilities/devtools.json`.
const PORTAL_LABEL: &str = "devtools";

/// Opens the dev portal, or focuses it if it is already open. Created
/// from Rust rather than JS so the main window doesn't need
/// `core:webview:allow-create-webview-window` in its capability set.
///
/// The main window calls this unconditionally and hides its own button
/// when the call is rejected — in a build without `devtools` this command
/// simply isn't registered, so "is the portal available" needs no second
/// flag to keep in sync.
///
/// `async` on purpose, and load-bearing on Windows. Tauri runs a
/// *synchronous* command on the main thread, which means a sync version of
/// this ran `build()` re-entrantly from inside WebView2's own IPC callback
/// — the Win32 window got created, because that part is synchronous, but
/// the WebView2 controller needs the message loop to pump and never
/// attached. `build()` still returned `Ok`, so the portal came up as a bare
/// white window with no webview in it at all: no page, no context menu,
/// and `open_devtools()` below silently doing nothing. macOS never showed
/// it because WKWebView is created synchronously.
///
/// An async command runs on the async runtime instead, so `build()`
/// dispatches to the event loop from off the main thread and waits for it
/// properly.
#[tauri::command]
pub async fn dev_open_portal(
    app: tauri::AppHandle,
    recording_id: Option<i64>,
) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    // The portal routes off `location.hash`, so a target is a fragment. Built
    // here from an **integer** rather than taking a path from the caller:
    // nothing string-shaped then crosses into a URL, and there is no escaping
    // to get wrong.
    let fragment = recording_id
        .map(|id| format!("#/library/{id}"))
        .unwrap_or_default();

    if let Some(existing) = app.get_webview_window(PORTAL_LABEL) {
        existing.show().map_err(|e| e.to_string())?;
        existing.set_focus().map_err(|e| e.to_string())?;
        // Already open, so the fragment has to be pushed rather than built
        // into the URL. `hashchange` is what the portal's router listens on,
        // and assigning the same value fires nothing at all.
        //
        // That is *not* the same as "the panel is already showing that
        // recording", which is what this used to assume. The Library panel's
        // own list moves its selection without touching the hash, so the hash
        // can name 12 while the panel shows 7 — and then this button would
        // assign an identical value, fire nothing, and read as dead. The
        // synthetic event forces the route in exactly that case.
        if !fragment.is_empty() {
            existing
                .eval(format!(
                    "if (location.hash === '{fragment}')                        window.dispatchEvent(new HashChangeEvent('hashchange'));                      else location.hash = '{fragment}';"
                ))
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    let window =
        WebviewWindowBuilder::new(
            &app,
            PORTAL_LABEL,
            WebviewUrl::App(format!("dev.html{fragment}").into()),
        )
            .title("ninja-recorder — dev portal")
            .inner_size(1280.0, 860.0)
            .min_inner_size(900.0, 600.0)
            .build()
            .map_err(|e| e.to_string())?;

    // Opened with the window rather than left to a right-click. The case
    // that most needs a console is the page failing to load at all, and a
    // blank webview gives you nothing to right-click on — so the portal
    // that cannot explain itself is precisely the one where the inspector
    // is unreachable. Relies on tauri's own `devtools` feature, which this
    // crate's `devtools` feature pulls in (Cargo.toml); a release build
    // has no inspector at all without it.
    window.open_devtools();
    Ok(())
}

/// Tells the frontend the VOD library changed. Every dev command that
/// writes to `recordings`/`markers`/`samples` calls this, so the main
/// window's library view stays in step with whatever the portal does to
/// it without the user reloading anything.
pub(crate) fn notify_library_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    if let Err(e) = app.emit(crate::LIBRARY_CHANGED_EVENT, ()) {
        eprintln!("[dev] failed to emit library-changed: {e}");
    }
}
