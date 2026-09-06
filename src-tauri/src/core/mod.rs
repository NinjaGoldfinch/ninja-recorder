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

use crate::db;
use crate::lcu;
use crate::recorder::audio::{AudioInputDevice, AudioPreset};
use crate::recorder::{RecordConfig, Recorder};
use crate::state_machine;
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
    /// The bundled (or locally installed) ffmpeg, if there is one. Optional
    /// by design — a failed CI download degrades stem extraction and the
    /// faststart remux rather than breaking recording.
    pub ffmpeg: Option<PathBuf>,
    /// Type-erased so this module stays `tauri`-free; see the module header.
    /// `None` simply means nothing is emitted, which is what the unit tests
    /// want.
    on_library_changed: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Ctx {
    pub fn new(
        recorder: Arc<Mutex<Box<dyn Recorder>>>,
        supervisor: Arc<state_machine::Supervisor>,
        db: Arc<db::Db>,
        recordings_dir: PathBuf,
        ffmpeg: Option<PathBuf>,
    ) -> Self {
        Self {
            recorder,
            supervisor,
            db,
            recordings_dir,
            ffmpeg,
            on_library_changed: None,
        }
    }

    /// Called once from `lib.rs`'s `setup`, after the app is built — the
    /// same shape as `Supervisor::set_library_changed_notifier`, and for
    /// the same reason.
    pub fn set_library_changed_notifier(&mut self, notify: Box<dyn Fn() + Send + Sync>) {
        self.on_library_changed = Some(notify);
    }

    fn notify_library_changed(&self) {
        if let Some(notify) = self.on_library_changed.as_ref() {
            notify();
        }
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
    db::reconcile::reconcile(&ctx.db, &ctx.recordings_dir).map_err(|e| e.to_string())
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

pub fn set_ui_pref(ctx: &Ctx, key: &str, value: &str) -> Result<(), String> {
    ctx.db.set_ui_pref(key, value).map_err(|e| e.to_string())
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
    recording_path: &str,
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
        Path::new(recording_path),
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
