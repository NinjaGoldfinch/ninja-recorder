//! Every command's actual logic, with no `tauri::` in a single signature.
//!
//! Until now these bodies lived in `lib.rs` as `#[tauri::command]`
//! functions, which meant the only way to run one was for a webview to
//! invoke it. That is a problem for the process split
//! ([DEVELOPMENT.md §12](../../../DEVELOPMENT.md)): the recorder daemon has
//! no webview, and Tauri v2 offers no way to call a registered command by
//! name from Rust. So the logic moves here, behind a plain `Ctx`, and
//! `lib.rs` keeps only thin `#[tauri::command]` wrappers over it.
//!
//! **Nothing in this module may name a `tauri` type.** That is not tidiness:
//! this module is unit-testable, and `state_machine::supervisor` documents
//! at length what happens when Wry becomes reachable from a module with
//! tests — the Win32 GUI stack lands in the `cargo test` binary, which has
//! no application manifest, and the binary dies at load with
//! `STATUS_ENTRYPOINT_NOT_FOUND`. Paths that used to come from
//! `AppHandle` are resolved once at startup and handed over in `Ctx`; the
//! one place that needs to emit a Tauri event does it through a type-erased
//! closure, exactly as `Supervisor::set_library_changed_notifier` does.
//!
//! Two commands deliberately stay behind in `lib.rs` rather than moving
//! here: `open_recordings_folder` and the dev portal's `dev_open_portal`.
//! Both drive the desktop shell — an opener call and a window — which only
//! the UI process can meaningfully do.

mod dispatch;

#[cfg_attr(not(feature = "devtools"), allow(unused_imports))]
pub use dispatch::{command_names, dispatch, dispatch_blocking, is_async_command};

use crate::warn;
use crate::db;
use crate::lcu;
use crate::recorder::audio::{AudioInputDevice, AudioPreset};
use crate::recorder::{RecordConfig, Recorder};
use crate::state_machine;
use crate::update::{self, CheckResult, UpdateRequest, UpdateStatus};
use crate::{audio_tracks, retention};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Everything a command needs, resolved once at startup.
///
/// Deliberately owns `recordings_dir` and `ffmpeg` as plain paths rather
/// than re-deriving them from an `AppHandle` per call, which is what the
/// three commands that used to take one were doing.
pub struct Ctx {
    pub recorder: Arc<Mutex<Box<dyn Recorder>>>,
    pub supervisor: Arc<state_machine::Supervisor>,
    pub db: Arc<db::Db>,
    pub recordings_dir: PathBuf,
    /// Where Data Dragon art is cached. Nothing is bundled and nothing is
    /// pre-fetched: files land here the first time a champion is asked for
    /// (`crate::ddragon`).
    pub assets_dir: PathBuf,
    /// The bundled (or locally installed) ffmpeg, if there is one. Optional
    /// by design — a failed CI download degrades stem extraction and the
    /// faststart remux rather than breaking recording.
    pub ffmpeg: Option<PathBuf>,
    /// Type-erased so this module stays `tauri`-free; see the module header.
    /// `None` simply means nothing is emitted, which is what the unit tests
    /// want.
    on_library_changed: Option<Box<dyn Fn() + Send + Sync>>,
    /// Start-on-login control, behind a trait for the same reason: the only
    /// implementation wraps `tauri-plugin-autostart`, whose manager is
    /// reached through an `AppHandle`.
    ///
    /// **`None` means the tests cannot reach the real registry.** `Ctx::new`
    /// leaves it unset, so `set_autostart` in a unit test fails loudly
    /// instead of writing a `Run` entry on whatever machine ran `cargo
    /// test` — which on a developer's Windows box would be an app that
    /// starts itself on login forever after.
    autostart: Option<Box<dyn Autostart>>,
    /// The last thing the background update check found, or `Pending` until
    /// something writes to it.
    ///
    /// A cell rather than a return value because the check is *async* and
    /// every command here is not. Keeping the network half in `lib.rs` — the
    /// one place that can hold an `AppHandle` — and leaving this side a plain
    /// read is what lets `every_command_round_trips` exercise the update
    /// commands without the test suite reaching GitHub.
    ///
    /// Only the raw finding lives here. Whether it is *installable* is
    /// recomputed per read against live state, because that answer changes
    /// while nothing here does (`crate::update::decide`).
    update: Mutex<CheckResult>,
    /// Asks the background half to check, or to install. Type-erased for the
    /// same reason as `on_library_changed`.
    ///
    /// `None` — which is what `Ctx::new` leaves — means no updater in this
    /// build, and `install_update` refuses loudly rather than pretending. The
    /// unit tests rely on that: an `install_update` that worked under `cargo
    /// test` would try to restart the test binary into an installer.
    on_update_request: Option<Box<dyn Fn(UpdateRequest) + Send + Sync>>,
}

impl Ctx {
    pub fn new(
        recorder: Arc<Mutex<Box<dyn Recorder>>>,
        supervisor: Arc<state_machine::Supervisor>,
        db: Arc<db::Db>,
        recordings_dir: PathBuf,
        assets_dir: PathBuf,
        ffmpeg: Option<PathBuf>,
    ) -> Self {
        Self {
            recorder,
            supervisor,
            db,
            recordings_dir,
            assets_dir,
            ffmpeg,
            on_library_changed: None,
            autostart: None,
            update: Mutex::new(CheckResult::Pending),
            on_update_request: None,
        }
    }

    /// Called once from `lib.rs`'s `setup`, after the app is built — the
    /// same shape as `Supervisor::set_library_changed_notifier`, and for
    /// the same reason.
    pub fn set_library_changed_notifier(&mut self, notify: Box<dyn Fn() + Send + Sync>) {
        self.on_library_changed = Some(notify);
    }

    /// Called once from `lib.rs`'s `setup`, for the same reason as
    /// `set_library_changed_notifier`: the implementation needs an
    /// `AppHandle`, which cannot be named here.
    pub fn set_autostart(&mut self, autostart: Box<dyn Autostart>) {
        self.autostart = Some(autostart);
    }

    /// Called once from `lib.rs`'s `setup`, for the same reason as
    /// `set_library_changed_notifier`.
    pub fn set_update_requester(&mut self, request: Box<dyn Fn(UpdateRequest) + Send + Sync>) {
        self.on_update_request = Some(request);
    }

    /// Records what a check found. Called from the background task in
    /// `lib.rs`, never from a command — hence `&self` and the mutex.
    pub fn set_update_check_result(&self, found: CheckResult) {
        match self.update.lock() {
            Ok(mut cell) => *cell = found,
            // A poisoned lock here means a panic while holding it, which
            // nothing in this path can do. Dropping the result loses one
            // check; the next one is six hours away and the manual button
            // is always there.
            Err(e) => warn!("update", "could not record the check result: {e}"),
        }
    }

    fn notify_library_changed(&self) {
        if let Some(notify) = self.on_library_changed.as_ref() {
            notify();
        }
    }
}

/// `settings_kv` key holding the `CloseAction`. A missing key means "use the
/// default", which is how every pref in that table works — adding one needs no
/// migration (DEVELOPMENT.md §5.1).
pub const CLOSE_ACTION_KEY: &str = "closeAction";

/// What the main window's close button does.
///
/// The default is `CloseWindow` rather than `Hide` because a *hidden* window
/// keeps WebView2 fully resident and reclaims nothing; destroying it while the
/// process lives on is what actually gets the idle footprint down, and the
/// recording is unaffected either way (DEVELOPMENT.md §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloseAction {
    /// Destroy the webview, keep the process and the recording running.
    #[default]
    CloseWindow,
    /// Keep the webview resident so reopening is instant. Costs its full
    /// memory the whole time.
    Hide,
    /// Quit everything, finalizing an in-flight recording first.
    Quit,
}

impl CloseAction {
    /// Parses the stored preference.
    ///
    /// Anything unrecognised falls back to the default rather than erroring.
    /// `settings_kv` is schemaless and shared across versions, so a build that
    /// is *older* than the one that wrote the value will read a variant it has
    /// never heard of — a downgrade must not brick the close button.
    pub fn from_pref(value: Option<&str>) -> Self {
        match value {
            Some("hide") => CloseAction::Hide,
            Some("quit") => CloseAction::Quit,
            Some("close-window") => CloseAction::CloseWindow,
            _ => CloseAction::default(),
        }
    }

    /// The string written back to `settings_kv`; round-trips `from_pref`.
    ///
    /// Test-only: the settings *form* writes these values, from TypeScript,
    /// so nothing in Rust ever needs them at runtime. Kept because it is
    /// what pins `from_pref` to the exact strings the frontend sends, and
    /// clippy runs without `--all-targets`, so an unused non-test method
    /// would fail `-D warnings`.
    #[cfg(test)]
    pub fn as_pref(self) -> &'static str {
        match self {
            CloseAction::CloseWindow => "close-window",
            CloseAction::Hide => "hide",
            CloseAction::Quit => "quit",
        }
    }
}

/// The user's close-button preference, or the default if unset or unreadable.
///
/// A database error is treated as "unset": this is read on the window-close
/// path, where failing would leave the user unable to close the window at all.
pub fn close_action(ctx: &Ctx) -> CloseAction {
    let prefs = match ctx.db.get_ui_prefs() {
        Ok(prefs) => prefs,
        Err(e) => {
            warn!("core", "could not read {CLOSE_ACTION_KEY}, using the default: {e}");
            return CloseAction::default();
        }
    };
    CloseAction::from_pref(prefs.get(CLOSE_ACTION_KEY).map(String::as_str))
}

/// `settings_kv` keys for the notification preferences. All default to
/// something sensible when absent, so none of them needs a migration.
pub const NOTIFY_MASTER_KEY: &str = "notifications";
pub const NOTIFY_STARTED_KEY: &str = "notifyRecordingStarted";
pub const NOTIFY_FINISHED_KEY: &str = "notifyRecordingFinished";
pub const NOTIFY_FAILED_KEY: &str = "notifyRecordingFailed";
/// Set once the "still running in the tray" notice has been shown.
pub const NOTICE_CLOSE_TO_TRAY_KEY: &str = "notice.closeToTray.seen";

/// What a notification is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    /// A recording began. Off by default — the header already shows it, and
    /// the user is in-game and does not want a popup over it.
    RecordingStarted,
    /// A VOD was written. On by default: this is the one piece of feedback
    /// worth having when the window is closed.
    RecordingFinished,
    /// Recording failed, or the disk is full. On by default, and the only
    /// kind the user cannot find out about any other way while in a game.
    RecordingFailed,
    /// The one-time "we are still running in the tray" notice.
    CloseToTray,
}

/// Which notifications the user wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPrefs {
    master: bool,
    started: bool,
    finished: bool,
    failed: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self {
            master: true,
            started: false,
            finished: true,
            failed: true,
        }
    }
}

impl NotificationPrefs {
    pub fn from_prefs(prefs: &HashMap<String, String>) -> Self {
        let defaults = Self::default();
        let read = |key: &str, default: bool| match prefs.get(key).map(String::as_str) {
            Some("on") => true,
            Some("off") => false,
            // Unrecognised or absent: keep the default. Same reasoning as
            // `CloseAction::from_pref` — this table is schemaless and shared
            // across versions.
            _ => default,
        };
        Self {
            master: read(NOTIFY_MASTER_KEY, defaults.master),
            started: read(NOTIFY_STARTED_KEY, defaults.started),
            finished: read(NOTIFY_FINISHED_KEY, defaults.finished),
            failed: read(NOTIFY_FAILED_KEY, defaults.failed),
        }
    }

    /// The master switch gates everything, including the one-time notice —
    /// turning notifications off has to mean off.
    pub fn allows(&self, kind: NotifyKind) -> bool {
        if !self.master {
            return false;
        }
        match kind {
            NotifyKind::RecordingStarted => self.started,
            NotifyKind::RecordingFinished => self.finished,
            NotifyKind::RecordingFailed => self.failed,
            // Not separately configurable: it fires at most once, and a
            // "reset one-time notices" action is what re-arms it.
            NotifyKind::CloseToTray => true,
        }
    }
}

/// The user's notification preferences, or the defaults if unreadable.
pub fn notification_prefs(ctx: &Ctx) -> NotificationPrefs {
    match ctx.db.get_ui_prefs() {
        Ok(prefs) => NotificationPrefs::from_prefs(&prefs),
        Err(e) => {
            warn!("core", "could not read notification prefs, using defaults: {e}");
            NotificationPrefs::default()
        }
    }
}

/// Whether a one-time notice has already been shown.
///
/// An **empty value counts as unseen**, which is what makes "reset one-time
/// notices" possible: `set_ui_pref` can only write, there is no delete
/// command, and adding one would mean editing the dispatch table and the dev
/// registry for a button. Blanking the key is the reset.
///
/// Errs on the side of "already shown" if the database can't be read — staying
/// quiet beats nagging on every close.
pub fn notice_seen(ctx: &Ctx, key: &str) -> bool {
    match ctx.db.get_ui_prefs() {
        Ok(prefs) => prefs.get(key).is_some_and(|value| !value.is_empty()),
        Err(_) => true,
    }
}

/// Records that a one-time notice has been shown.
pub fn mark_notice_seen(ctx: &Ctx, key: &str) {
    if let Err(e) = ctx.db.set_ui_pref(key, "1") {
        warn!("core", "could not record notice {key} as seen: {e}");
    }
}

#[derive(serde::Serialize)]
pub struct DiskUsage {
    pub total_bytes: i64,
    pub recording_count: i64,
    pub free_bytes: i64,
}

#[derive(serde::Serialize)]
pub struct LcuStatus {
    pub connected: bool,
    pub phase: Option<String>,
    pub summoner: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------- recorder

pub fn start_recording(ctx: &Ctx) -> Result<(), String> {
    if !retention::has_room_to_record(&ctx.recordings_dir) {
        return Err("Not enough free disk space to start recording".to_string());
    }
    let config = RecordConfig {
        output_dir: ctx.recordings_dir.clone(),
        file_stem: format!("recording-{}", chrono_stamp()),
        audio: ctx.db.get_audio_preset().map_err(|e| e.to_string())?,
    };
    ctx.recorder
        .lock()
        .map_err(|e| e.to_string())?
        .start(config)
        .map_err(|e| e.to_string())
}

pub fn stop_recording(ctx: &Ctx) -> Result<String, String> {
    let output = ctx
        .recorder
        .lock()
        .map_err(|e| e.to_string())?
        .stop()
        .map_err(|e| e.to_string())?;
    Ok(output.path.display().to_string())
}

pub fn is_recording(ctx: &Ctx) -> Result<bool, String> {
    Ok(ctx
        .recorder
        .lock()
        .map_err(|e| e.to_string())?
        .is_recording())
}

// ----------------------------------------------------------------- library

pub fn list_recordings(ctx: &Ctx) -> Result<Vec<db::RecordingRow>, String> {
    ctx.db.list_recordings().map_err(|e| e.to_string())
}

/// Re-runs folder-scan reconciliation on demand (also runs once at
/// startup). DEVELOPMENT.md §4 — "the library must survive users touching
/// the folder."
pub fn rescan_recordings(ctx: &Ctx) -> Result<db::reconcile::ReconcileReport, String> {
    db::reconcile::reconcile(&ctx.db, &ctx.recordings_dir, ctx.ffmpeg.as_deref())
        .map_err(|e| e.to_string())
}

/// Labels the recordings that predate the metadata pipeline, by matching
/// them against the client's match history on the clock.
///
/// Manual on purpose — never on startup. It is a bulk read against the
/// user's running client, and when to do that is theirs to choose. The
/// frontend refreshes the library itself when this returns rather than the
/// backend emitting `library-changed`: the caller is right there, and one
/// refresh at the end beats one per patched row.
///
/// Async, so `rpc` awaits it on an async worker rather than handing it to
/// `spawn_blocking` — and it does its SQLite work there, which the dispatch
/// table's own comment warns about. Accepted rather than wrapped: this runs
/// when a person clicks a button in settings, at most once in a while, and
/// the awaits between the writes are the long part. Making it a background
/// job with progress would be the fix if it ever walks thousands of rows.
pub async fn backfill_match_metadata(ctx: &Ctx) -> Result<crate::backfill::BackfillReport, String> {
    crate::backfill::run(&ctx.db).await
}

/// Cached art for a page of rows — champions, items, summoner spells and
/// runes — fetched on first use.
///
/// Returns paths for the frontend to hand to `convertFileSrc`. Anything
/// that could not be resolved is **absent** rather than null: offline, an
/// unknown id, an unwritable cache. None of those is an error, because the
/// row renders the text it always did, which is why this cannot fail
/// loudly.
///
/// One call for a whole page rather than one per icon: a row carries up to
/// seven items, two spells, three runes and a champion, so a library of
/// forty rows would otherwise be several hundred round trips.
pub async fn resolve_icons(
    ctx: &Ctx,
    request: crate::ddragon::IconRequest,
) -> Result<crate::ddragon::IconSet, String> {
    Ok(crate::ddragon::resolve_icons(&ctx.assets_dir, &request).await)
}

/// Markers for the review timeline.
pub fn get_recording_markers(ctx: &Ctx, recording_id: i64) -> Result<Vec<db::MarkerRow>, String> {
    ctx.db.get_markers(recording_id).map_err(|e| e.to_string())
}

/// Advantage-curve samples for the review timeline's graph. Returns an
/// empty vec for any recording made before sampling existed — the frontend
/// treats that as "no metric data" rather than an error.
pub fn get_recording_samples(ctx: &Ctx, recording_id: i64) -> Result<Vec<db::SampleRow>, String> {
    ctx.db.get_samples(recording_id).map_err(|e| e.to_string())
}

/// Usage summary for the library UI (DEVELOPMENT.md §6) — shown
/// alongside the retention policy so nothing gets deleted as a surprise.
pub fn get_disk_usage(ctx: &Ctx) -> Result<DiskUsage, String> {
    let total_bytes = ctx.db.total_size_bytes().map_err(|e| e.to_string())?;
    let recording_count = ctx.db.list_recordings().map_err(|e| e.to_string())?.len() as i64;
    let free_bytes = retention::free_space_bytes(&ctx.recordings_dir).unwrap_or(0) as i64;
    Ok(DiskUsage {
        total_bytes,
        recording_count,
        free_bytes,
    })
}

pub fn get_recordings_dir(ctx: &Ctx) -> String {
    ctx.recordings_dir.display().to_string()
}

/// User-initiated delete of a single recording — file first, then row.
/// Unlike retention's sweep this reports a file it couldn't remove instead
/// of dropping the row anyway, so the library still shows what's on disk.
pub fn delete_recording(ctx: &Ctx, recording_id: i64) -> Result<(), String> {
    let row = ctx
        .db
        .get_recording(recording_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("recording {recording_id} not found"))?;
    retention::delete_recording_and_file(&ctx.db, &row).map_err(|e| e.to_string())
}

pub fn set_pinned(ctx: &Ctx, recording_id: i64, pinned: bool) -> Result<(), String> {
    ctx.db
        .set_pinned(recording_id, pinned)
        .map_err(|e| e.to_string())
}

// --------------------------------------------------------------- retention

pub fn get_retention_policy(ctx: &Ctx) -> Result<db::RetentionPolicy, String> {
    ctx.db.get_retention_policy().map_err(|e| e.to_string())
}

/// Saves the policy and immediately re-enforces it — otherwise a
/// newly-tightened limit wouldn't take effect until the next finalize or
/// app restart, which would leave the UI's own usage number stale.
pub fn set_retention_policy(
    ctx: &Ctx,
    policy: db::RetentionPolicy,
) -> Result<retention::EnforcementReport, String> {
    ctx.db
        .set_retention_policy(&policy)
        .map_err(|e| e.to_string())?;
    let report = retention::enforce_now(&ctx.db, &policy).map_err(|e| e.to_string())?;
    if !report.deleted.is_empty() {
        ctx.notify_library_changed();
    }
    Ok(report)
}

/// Dry run for the settings form: what `set_retention_policy` would delete
/// if saved with `policy`. Nothing is written.
pub fn preview_retention_policy(
    ctx: &Ctx,
    policy: db::RetentionPolicy,
) -> Result<retention::EnforcementReport, String> {
    retention::preview(&ctx.db, &policy).map_err(|e| e.to_string())
}

// ------------------------------------------------------------- preferences

/// Every UI preference in one call — the frontend reads the whole set at
/// boot. Missing keys are absent rather than defaulted: the defaults live
/// in the frontend, so adding a pref needs no migration.
pub fn get_ui_prefs(ctx: &Ctx) -> Result<HashMap<String, String>, String> {
    ctx.db.get_ui_prefs().map_err(|e| e.to_string())
}

pub fn set_ui_pref(ctx: &Ctx, key: String, value: String) -> Result<(), String> {
    ctx.db.set_ui_pref(&key, &value).map_err(|e| e.to_string())
}

// -------------------------------------------------------------- autostart

/// Registering the app to run at login.
///
/// A trait rather than a direct call because the only implementation wraps
/// `tauri-plugin-autostart`, whose manager comes off an `AppHandle` — and
/// nothing here may name a `tauri` type (module header). It also makes the
/// commands below testable without a login item existing anywhere.
///
/// Every method returns the platform's own answer. On Windows all three are
/// registry operations against `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
/// which the user can also edit from Task Manager's Startup tab or a policy
/// can forbid outright, so none of them is assumed to succeed.
pub trait Autostart: Send + Sync {
    /// Whether a login entry for *this* executable exists right now.
    fn is_enabled(&self) -> Result<bool, String>;
    fn enable(&self) -> Result<(), String>;
    fn disable(&self) -> Result<(), String>;
}

/// What the settings screen needs to render the start-on-login row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AutostartStatus {
    /// The platform's answer, re-read after any change — never what was
    /// asked for.
    pub enabled: bool,
    /// False when this build has no autostart control at all, which is the
    /// signal to show the row disabled rather than a checkbox that lies.
    pub supported: bool,
}

/// Whether the app is registered to start on login.
///
/// **Read live from the platform, not mirrored into `settings_kv`.** Every
/// other preference here is ours alone, but this one has a second owner: the
/// user can delete the entry from Task Manager's Startup tab, an installer or
/// a group policy can remove it, and a copy in SQLite would then be a
/// checkbox confidently describing something that will not happen. The
/// registry is the truth; this just reports it.
pub fn get_autostart(ctx: &Ctx) -> Result<AutostartStatus, String> {
    let Some(autostart) = ctx.autostart.as_ref() else {
        return Ok(AutostartStatus {
            enabled: false,
            supported: false,
        });
    };
    Ok(AutostartStatus {
        enabled: autostart.is_enabled()?,
        supported: true,
    })
}

/// Turns start-on-login on or off, and reports what the platform says
/// afterwards.
///
/// The re-read is the point. A `Run` entry write can be silently overruled —
/// by policy, by permissions, by another copy of the app owning the same key
/// — and returning `enabled` on the strength of "the call didn't error" would
/// leave the checkbox ticked for a machine that will never launch us.
pub fn set_autostart(ctx: &Ctx, enabled: bool) -> Result<AutostartStatus, String> {
    let autostart = ctx
        .autostart
        .as_ref()
        .ok_or("start-on-login is not available in this build")?;

    if enabled {
        autostart.enable()?;
    } else {
        autostart.disable()?;
    }

    Ok(AutostartStatus {
        enabled: autostart.is_enabled()?,
        supported: true,
    })
}

// ---------------------------------------------------------------- updates

/// Whether the app can safely exit into an installer right now, as the two
/// halves that know see it.
///
/// A poisoned recorder lock counts as *recording*. It cannot actually happen
/// — nothing in the recorder path panics while holding it — but the failure
/// mode if it ever did is asymmetric: guessing "idle" wrong ends a game's
/// capture, and guessing "busy" wrong costs a click.
fn update_gate(ctx: &Ctx) -> Result<(), String> {
    let busy = is_recording(ctx).unwrap_or(true);
    update::installable(&ctx.supervisor.status().state, busy)
}

/// What the About block renders.
///
/// The installability half is recomputed here rather than stored with the
/// check: the check runs every six hours, and whether a game is in progress
/// changes rather more often than that.
pub fn get_update_status(ctx: &Ctx) -> Result<UpdateStatus, String> {
    let found = ctx.update.lock().map_err(|e| e.to_string())?.clone();
    let busy = is_recording(ctx).unwrap_or(true);
    Ok(update::decide(&found, &ctx.supervisor.status().state, busy))
}

/// Asks for a check now, rather than waiting for the six-hourly one.
///
/// Returns as soon as the request is handed over — the result arrives later,
/// in the cell, and the frontend hears about it through the
/// `update-status-changed` event. Nothing here blocks on the network.
pub fn check_for_update(ctx: &Ctx) -> Result<(), String> {
    let request = ctx
        .on_update_request
        .as_ref()
        .ok_or("updates are not available in this build")?;
    request(UpdateRequest::Check);
    Ok(())
}

/// Downloads the offered installer and hands the machine over to it.
///
/// **This ends the process.** The gate is re-checked here and not merely in
/// the UI, because the button was rendered at some earlier moment and a game
/// can start between a glance and a click.
pub fn install_update(ctx: &Ctx) -> Result<(), String> {
    let request = ctx
        .on_update_request
        .as_ref()
        .ok_or("updates are not available in this build")?;
    if let Err(why) = update_gate(ctx) {
        return Err(format!("not installing an update while {why}"));
    }
    request(UpdateRequest::Install);
    Ok(())
}

/// The audio capture preset, read and written through `serde` rather than
/// as a raw `settings_kv` string like `theme` is.
///
/// The distinction matters: a bad theme value looks wrong, but a bad audio
/// preset changes what gets recorded — including whether the microphone is
/// live. Validating on this side keeps that decision next to the recorder
/// that acts on it instead of trusting the frontend.
pub fn get_audio_preset(ctx: &Ctx) -> Result<AudioPreset, String> {
    ctx.db.get_audio_preset().map_err(|e| e.to_string())
}

pub fn set_audio_preset(ctx: &Ctx, preset: AudioPreset) -> Result<(), String> {
    // Rejected here rather than at record time: the user is looking at the
    // settings screen right now and can act on the message. Only a `Custom`
    // layout can actually fail this.
    preset.layout().validate()?;
    ctx.db.set_audio_preset(&preset).map_err(|e| e.to_string())
}

// -------------------------------------------------------------------- audio

/// Audio inputs for the microphone picker, default first. Empty off Windows,
/// where nothing can be captured anyway.
///
/// Blocking, deliberately: the caller decides how to get off the current
/// thread. The Tauri wrapper uses `spawn_blocking`; the daemon will do the
/// same from its own per-request task, and neither has to agree with the
/// other about which async runtime is in play.
pub fn list_audio_inputs() -> Result<Vec<AudioInputDevice>, String> {
    crate::recorder::devices::list_audio_inputs()
}

/// Extracts one audio track out of a recording into a standalone file the
/// review player can play alongside the (muted) video.
///
/// This exists because WebView2 gives us no way to select among the audio
/// tracks of a single `<video>`: `HTMLMediaElement.audioTracks` sits behind
/// an experimental Blink flag on a runtime whose version we don't control.
/// Track 0 is the combined mix and needs none of this — only stem selection
/// comes through here, so the cost is paid by the rare case.
///
/// Cheap despite appearances: `-c copy` on one audio stream rewrites tens of
/// megabytes, not the multi-gigabyte video. Cached, so switching back to a
/// stem already extracted is free. See DEVELOPMENT.md §2.5. Blocking, for
/// the same reason as `list_audio_inputs`.
pub fn extract_audio_track(
    ctx: &Ctx,
    recording_path: String,
    track_index: usize,
) -> Result<String, String> {
    if track_index == 0 {
        return Err("track 0 is the combined mix and plays from the video itself".into());
    }
    let ffmpeg = ctx
        .ffmpeg
        .as_ref()
        .ok_or("ffmpeg was not bundled with this build, so audio stems can't be extracted")?;

    audio_tracks::extract(
        ffmpeg,
        &ctx.recordings_dir,
        Path::new(&recording_path),
        track_index,
    )
    .map(|path| path.display().to_string())
}

// ------------------------------------------------------------------ status

/// Current game state and the most recently finished recording (if any).
/// The real, always-on driver is `Supervisor::start`, spawned once at app
/// startup — this just reads its status.
pub fn game_state_status(ctx: &Ctx) -> state_machine::SupervisorStatus {
    ctx.supervisor.status()
}

/// One-shot LCU status check: is the client running, and if so, what's its
/// current gameflow phase / summoner. The state machine is what keeps this
/// live continuously via `lcu::gameflow::watch`; this is a smoke test that
/// the client + auth + parsing work.
///
/// The one genuinely async command, and the only one that touches the
/// network — it takes no `Ctx` because it discovers the lockfile itself.
pub async fn lcu_status() -> LcuStatus {
    let unreachable = |error: Option<String>| LcuStatus {
        connected: false,
        phase: None,
        summoner: None,
        error,
    };

    let lockfile = match lcu::lockfile::discover() {
        Ok(Some(lf)) => lf,
        Ok(None) => return unreachable(None),
        Err(e) => return unreachable(Some(e.to_string())),
    };

    let client = match lcu::LcuHttpClient::new(&lockfile) {
        Ok(c) => c,
        Err(e) => return unreachable(Some(e.to_string())),
    };

    let phase = client
        .get_json::<lcu::GameflowPhase>("/lol-gameflow/v1/gameflow-phase")
        .await;
    let summoner = client
        .get_json::<lcu::match_data::CurrentSummoner>("/lol-summoner/v1/current-summoner")
        .await;

    LcuStatus {
        connected: true,
        phase: phase.ok().map(|p| format!("{p:?}")),
        summoner: summoner.ok().and_then(|s| s.display()),
        error: None,
    }
}

/// Timestamp for default filenames. Only used by the manual `start_recording`
/// path — the state machine has its own copy, since it drives recording from
/// gameflow events rather than a button click.
fn chrono_stamp() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
