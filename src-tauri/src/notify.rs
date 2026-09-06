//! Desktop notifications.
//!
//! Like `tray.rs`, this carries **no tests and should never grow any** — it is
//! reachable only from `lib.rs`'s `run()`, which is dead code in a test build
//! and gets stripped, keeping the Win32 GUI import stack out of the `cargo
//! test` binary. The testable half — which notifications are enabled, and
//! whether a one-time notice has fired — lives in `core`.
//!
//! **Everything here is best-effort and must stay that way.** A notification
//! that fails to show is a logged warning, never an error that propagates: it
//! is feedback about a recording, and it must not be able to affect the
//! recording. `?` does not belong in this file.
//!
//! ## What works where
//!
//! - **Windows, installed build.** The plugin sets the notification's
//!   `System.AppUserModel.ID` to the bundle identifier, which Windows resolves
//!   through the Start-menu shortcut the NSIS installer creates. This is the
//!   real path and it can only be checked on an installed build
//!   ([docs/windows-verification.md](../../docs/windows-verification.md)).
//! - **Windows, `cargo run` / `tauri dev`.** The plugin detects an exe under
//!   `target/debug` or `target/release` and deliberately *skips* the AUMID, so
//!   a toast may appear unattributed or not at all. Not a bug, and not worth
//!   working around.
//! - **macOS dev.** The plugin sets the application to `com.apple.Terminal`,
//!   so notifications do appear, attributed to Terminal. That makes the wiring
//!   testable here even though the Windows presentation cannot be.
//!
//! **Display-only, by design.** Making a toast *clickable* on Windows needs a
//! registered COM notification activator CLSID; a Start-menu shortcut alone
//! buys presentation, not activation callbacks. So nothing here promises that
//! clicking does anything — the tray icon is the way back into the app.

use crate::core::{self, Ctx, NotifyKind};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Shows a notification if the user's preferences allow that kind.
///
/// Reads the preferences per call rather than caching them: they are two
/// SQLite reads on a path that fires a handful of times per game, and a cache
/// would go stale the moment the settings form was touched.
pub(crate) fn notify(app: &AppHandle, ctx: &Ctx, kind: NotifyKind, title: &str, body: &str) {
    if !core::notification_prefs(ctx).allows(kind) {
        return;
    }
    show(app, title, body);
}

/// Shows the one-time "still running in the tray" notice, at most once ever.
///
/// The first time the window is closed the app appears to have quit while it
/// is in fact still recording. Saying so once is the difference between a
/// feature and a bug report.
pub(crate) fn close_to_tray_notice(app: &AppHandle, ctx: &Ctx) {
    if core::notice_seen(ctx, core::NOTICE_CLOSE_TO_TRAY_KEY) {
        return;
    }
    // Marked before showing, not after: if the notification backend is broken
    // we would otherwise retry on every single close forever.
    core::mark_notice_seen(ctx, core::NOTICE_CLOSE_TO_TRAY_KEY);
    notify(
        app,
        ctx,
        NotifyKind::CloseToTray,
        "ninja-recorder is still running",
        "It stays in the tray and keeps recording your games. Quit from the tray icon to stop it.",
    );
}

fn show(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("[notify] could not show a notification: {e}");
    }
}
