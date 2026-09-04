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
mod retention_api;
mod seed;
mod simulate;
mod sql;

// Glob re-exports, not a named list: `#[tauri::command]` expands to the
// function *plus* hidden `__cmd__*` / `__tauri_command_name_*` items that
// `generate_handler!` resolves through the same path, and naming only the
// function leaves those behind in the submodule.
pub use fixtures_api::*;
pub use info::*;
pub use retention_api::*;
pub use seed::*;
pub use simulate::*;
pub use sql::*;

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
#[tauri::command]
pub fn dev_open_portal(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    if let Some(existing) = app.get_webview_window(PORTAL_LABEL) {
        existing.show().map_err(|e| e.to_string())?;
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    WebviewWindowBuilder::new(&app, PORTAL_LABEL, WebviewUrl::App("dev.html".into()))
        .title("ninja-recorder — dev portal")
        .inner_size(1280.0, 860.0)
        .min_inner_size(900.0, 600.0)
        .build()
        .map_err(|e| e.to_string())?;
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
