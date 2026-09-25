//! Desktop notifications, from the process that knows. WS3 task 3.3.
//!
//! These belong to the daemon and always did: §3.1's ownership table says so,
//! and the reason is the whole point of the split. A notification exists to
//! tell someone what happened while they were not looking, which is precisely
//! when there is no window to look at.
//!
//! They were briefly missing. WS3.4 moved the supervisor out of the UI and its
//! event notifier went with it, and the daemon could not raise one because
//! `tauri-plugin-notification` needs an `AppHandle`. This is that notifier,
//! rebuilt on `notify-rust` directly, which is the crate the plugin wraps and
//! was already in the tree through it.
//!
//! ## Everything here is best-effort and must stay that way
//!
//! A notification that fails to show is a logged warning and nothing else. It
//! is feedback *about* a recording and must never be able to affect one. `?`
//! does not belong in this file, exactly as it does not in `crate::notify`.
//!
//! ## The AppUserModelID, and where a toast actually appears
//!
//! Windows attributes a toast to an application id, which it resolves through a
//! Start-menu shortcut. The plugin set that to the bundle identifier for an
//! installed build and deliberately skipped it for one running out of `target/`,
//! where there is no shortcut and the toast would be attributed to nothing.
//! `app_id` below does the same, for the same reason, because the rule is about
//! Windows rather than about Tauri.
//!
//! So: an installed build shows toasts, and a `cargo run` build on Windows may
//! show them unattributed or not at all. That is not a bug and not worth
//! working around ([docs/windows-verification.md](../../../docs/windows-verification.md)).
//!
//! ## No tests
//!
//! The decision half is `core::notification_prefs`, which is pure and tested
//! there. What is left here is a call into the OS, which has nothing to assert
//! about that would not be asserting that `notify-rust` works.

use crate::core::{self, Ctx, NotifyKind};
use crate::state_machine::SupervisorEvent;
use crate::warn;

/// Shows a notification if the user's preferences allow that kind.
///
/// Reads the preferences per call rather than caching them: they are two SQLite
/// reads on a path that fires a handful of times per game, and a cache would go
/// stale the moment the settings form was touched.
pub fn notify(ctx: &Ctx, kind: NotifyKind, title: &str, body: &str) {
    if !core::notification_prefs(ctx).allows(kind) {
        return;
    }

    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);

    // `app_id` exists only on Windows, because the concept does: on Linux the
    // desktop attributes a notification by the bus name it arrived on, and on
    // macOS by the bundle. This is the one place the daemon's notifications
    // differ by platform, and it is the crate's own `cfg` rather than a choice.
    #[cfg(target_os = "windows")]
    if let Some(id) = app_id() {
        notification.app_id(&id);
    }

    if let Err(e) = notification.show() {
        warn!("notify", "could not show a notification: {e}");
    }
}

/// The application id a toast is attributed to, or `None` when there is
/// nothing to attribute it to.
///
/// Windows-only, like the setter it feeds.
#[cfg(target_os = "windows")]
///
/// `None` for a binary running out of `target/debug` or `target/release`: that
/// build has no Start-menu shortcut, so the id would resolve to nothing. The
/// plugin made the same check and this keeps it.
fn app_id() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?.file_name()?.to_string_lossy().into_owned();
    if parent == "debug" || parent == "release" {
        return None;
    }
    Some(super::IDENTIFIER.to_string())
}

/// The supervisor's notifier: what a person is told about a game.
///
/// Installed in `daemon::start`, which is the daemon's equivalent of the
/// `lib.rs` setup this was lifted from. The bodies are unchanged from the
/// version that ran in the UI, because what is worth saying about a recording
/// did not change when the process did.
///
/// `LibraryChanged` is deliberately absent. In the UI it emitted a Tauri event;
/// here the contract sink already publishes what moved, and a second
/// announcement of the same fact is how two channels start to disagree.
pub fn on_supervisor_event(ctx: &Ctx, event: SupervisorEvent) {
    match event {
        SupervisorEvent::LibraryChanged => {}
        SupervisorEvent::RecordingStarted => notify(
            ctx,
            NotifyKind::RecordingStarted,
            "Recording started",
            "ninja-recorder is capturing this game.",
        ),
        SupervisorEvent::Finalized(finalized, problems) => {
            let name = std::path::Path::new(&finalized.path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| finalized.path.clone());
            // A recording that lost something says so **instead of** the
            // plain "Recording saved", so one game is one toast (#10). It is
            // a problem, so it is the "failed" preference that governs it;
            // someone who turned those off still gets the ordinary toast.
            if core::notification_prefs(ctx).allows(NotifyKind::RecordingFailed)
                && let Some((title, body)) = crate::recorder::problem::notification(
                    &name,
                    &problems,
                    crate::recorder::problem::windows_build(),
                )
            {
                notify(ctx, NotifyKind::RecordingFailed, &title, &body);
                return;
            }
            let markers = finalized.markers.len();
            // No champion or KDA here: those columns are still NULL on real
            // recordings (DEVELOPMENT.md §3.4), so the toast says what is
            // actually known.
            let body = if markers == 1 {
                format!("{name}: 1 marker")
            } else {
                format!("{name}: {markers} markers")
            };
            notify(ctx, NotifyKind::RecordingFinished, "Recording saved", &body);
        }
        SupervisorEvent::RecordingFailed(message) => {
            notify(ctx, NotifyKind::RecordingFailed, "Recording problem", &message)
        }
    }
}
